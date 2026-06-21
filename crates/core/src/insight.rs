//! Contrato **puro** de la capa de IA opcional (F12). El dominio NO conoce red,
//! async ni ningún proveedor: acá viven sólo la **construcción del prompt** y el
//! **parseo de la respuesta** (funciones puras, testeables sin key) más la
//! **estimación de costo en dinero**. La parte async/HTTP vive en el crate
//! `sdp-llm` detrás del trait `InsightProvider`.
//!
//! El prompt se arma una vez y sirve para cualquier modelo; el parseo tolera que
//! el modelo envuelva el JSON en texto o en bloques de código.

use serde::{Deserialize, Serialize};

use crate::cost::{CostEstimate, CostKind, CostLine};
use crate::ideas::VideoIdea;

/// Proveedor de IA elegible en runtime. El costo (US$) depende del proveedor y
/// modelo elegidos; los adaptadores HTTP concretos están en `sdp-llm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiProvider {
    Anthropic,
    Gemini,
}

/// El prompt construido, listo para cualquier modelo (system + user).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnhancePrompt {
    pub system: String,
    pub user: String,
}

/// Una idea **refinada por IA**: título pulido, gancho y un porqué. El origen
/// (`AiProvider`) lo conoce la capa de orquestación, no el dominio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiIdea {
    pub title: String,
    pub hook: String,
    pub rationale: String,
}

/// Por qué falló el parseo de la respuesta del modelo (la IA a veces no devuelve
/// JSON limpio): se distingue para que la UI muestre un error claro.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("la respuesta del modelo vino vacía")]
    Empty,
    #[error("no se encontró un arreglo JSON en la respuesta")]
    NoJsonArray,
    #[error("JSON inválido en la respuesta: {0}")]
    InvalidJson(String),
}

/// Tokens de salida que asumimos por idea al estimar el costo (techo prudente).
pub const OUTPUT_TOKENS_PER_IDEA: u64 = 80;
/// Aproximación de tokens por carácter (~4 chars/token) para estimar la entrada.
const CHARS_PER_TOKEN: usize = 4;
/// Margen fijo de tokens de salida sobre el presupuesto por idea (cierres de
/// JSON, espacios, una idea que se pase del techo). Evita truncar la respuesta
/// justo en el borde del array.
const OUTPUT_TOKENS_MARGIN: u64 = 256;
/// Piso del tope de salida: aunque haya 0/1 ideas, dejamos lugar para un array
/// JSON razonable.
const OUTPUT_TOKENS_FLOOR: u64 = 512;

/// Deriva el tope de tokens de salida (`max_tokens`) del **mismo presupuesto**
/// que usa el estimate de costo: `OUTPUT_TOKENS_PER_IDEA * ideas + margen`, con
/// un piso. Que el ejecutado coincida con lo estimado evita cobrar por una
/// respuesta que `max_tokens` garantiza truncada (ver auditoría P2).
pub fn max_output_tokens_for(idea_count: usize) -> u64 {
    let budget = OUTPUT_TOKENS_PER_IDEA * idea_count as u64 + OUTPUT_TOKENS_MARGIN;
    budget.max(OUTPUT_TOKENS_FLOOR)
}

/// Precio en `usd_micros` (millonésimas de dólar) por **1M de tokens**, por
/// proveedor/modelo. Verificar contra la tabla vigente del proveedor.
fn price_per_mtok(provider: AiProvider) -> (u64, u64) {
    match provider {
        // Anthropic Claude Opus 4.8: US$5 in / US$25 out por 1M.
        AiProvider::Anthropic => (5_000_000, 25_000_000),
        // Google Gemini 2.5 Flash (aprox.): US$0,30 in / US$2,50 out por 1M.
        AiProvider::Gemini => (300_000, 2_500_000),
    }
}

/// Construye el prompt para refinar ideas (F12). PURO. El system fija el rol y el
/// formato de salida (JSON); el user embebe las semillas heurísticas (F7).
pub fn build_ideas_prompt(ideas: &[VideoIdea]) -> EnhancePrompt {
    let system = "\
Sos un estratega de contenido para creadores de YouTube. Te paso ideas crudas \
detectadas en los comentarios de una comunidad. Para cada una, devolvé un título \
atractivo, un gancho de una frase y un porqué breve. Respondé EXCLUSIVAMENTE con \
un arreglo JSON, sin texto adicional ni bloques de código, con objetos de la \
forma {\"title\": string, \"hook\": string, \"rationale\": string}."
        .to_string();

    let mut user = String::from("Ideas crudas de la comunidad:\n");
    for (i, idea) in ideas.iter().enumerate() {
        user.push_str(&format!(
            "{}. tema \"{}\" (señal {:?}, {} comentarios de respaldo)\n",
            i + 1,
            idea.title_seed,
            idea.signal,
            idea.supporting_comment_ids.len()
        ));
    }
    EnhancePrompt { system, user }
}

/// Parsea la respuesta del modelo a `Vec<AiIdea>`. Tolera que el JSON venga
/// envuelto en texto o en un bloque ```` ```json ````: recorta desde el primer
/// `[` hasta el último `]`. PURO.
pub fn parse_ideas_response(raw: &str) -> Result<Vec<AiIdea>, ParseError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }
    let start = trimmed.find('[').ok_or(ParseError::NoJsonArray)?;
    let end = trimmed.rfind(']').ok_or(ParseError::NoJsonArray)?;
    if end < start {
        return Err(ParseError::NoJsonArray);
    }
    let json = &trimmed[start..=end];
    serde_json::from_str::<Vec<AiIdea>>(json).map_err(|e| ParseError::InvalidJson(e.to_string()))
}

/// Estima el costo **en dinero** (US$) de refinar `ideas` con IA (F12). Entrada
/// ≈ largo del prompt / 4; salida ≈ `OUTPUT_TOKENS_PER_IDEA` por idea. Siempre
/// `requires_confirmation = true` (gasta plata real). PURO.
pub fn estimate_ideas_ai(provider: AiProvider, ideas: &[VideoIdea]) -> CostEstimate {
    let prompt = build_ideas_prompt(ideas);
    let input_tokens = ((prompt.system.len() + prompt.user.len()) / CHARS_PER_TOKEN) as u64;
    let output_tokens = OUTPUT_TOKENS_PER_IDEA * ideas.len() as u64;

    let (in_per_mtok, out_per_mtok) = price_per_mtok(provider);
    let usd_micros =
        (input_tokens * in_per_mtok) / 1_000_000 + (output_tokens * out_per_mtok) / 1_000_000;
    let kind = CostKind::Money { usd_micros };

    CostEstimate {
        kind,
        requires_confirmation: true, // dinero real: siempre confirma
        breakdown: vec![CostLine {
            label: format!("IA {provider:?}: ~{input_tokens} in / ~{output_tokens} out tokens"),
            kind,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ideas::{DemandSignal, VideoIdea};

    fn idea(seed: &str) -> VideoIdea {
        VideoIdea {
            title_seed: seed.into(),
            signal: DemandSignal::Question,
            supporting_comment_ids: vec!["c1".into(), "c2".into()],
            score: 10,
            sample_quotes: vec!["¿cómo uso esto?".into()],
        }
    }

    #[test]
    fn build_prompt_embebe_las_semillas_y_pide_json() {
        let p = build_ideas_prompt(&[idea("signals"), idea("rxjs")]);
        assert!(p.system.contains("JSON"));
        assert!(p.user.contains("signals"));
        assert!(p.user.contains("rxjs"));
    }

    #[test]
    fn build_prompt_es_determinista() {
        let ideas = [idea("a"), idea("b")];
        assert_eq!(build_ideas_prompt(&ideas), build_ideas_prompt(&ideas));
    }

    #[test]
    fn parsea_json_limpio() {
        let raw = r#"[{"title":"T","hook":"H","rationale":"R"}]"#;
        let ideas = parse_ideas_response(raw).unwrap();
        assert_eq!(ideas.len(), 1);
        assert_eq!(ideas[0].title, "T");
    }

    #[test]
    fn parsea_json_envuelto_en_texto_o_fences() {
        let raw = "Claro, acá van:\n```json\n[{\"title\":\"T\",\"hook\":\"H\",\"rationale\":\"R\"}]\n```\n¡Suerte!";
        let ideas = parse_ideas_response(raw).unwrap();
        assert_eq!(ideas[0].hook, "H");
    }

    #[test]
    fn parseo_distingue_errores() {
        assert_eq!(parse_ideas_response("   "), Err(ParseError::Empty));
        assert_eq!(
            parse_ideas_response("no hay json acá"),
            Err(ParseError::NoJsonArray)
        );
        assert!(matches!(
            parse_ideas_response("[{roto}]"),
            Err(ParseError::InvalidJson(_))
        ));
    }

    #[test]
    fn estima_dinero_y_siempre_confirma() {
        let est = estimate_ideas_ai(AiProvider::Anthropic, &[idea("signals")]);
        assert!(est.requires_confirmation);
        match est.kind {
            CostKind::Money { usd_micros } => assert!(usd_micros > 0),
            other => panic!("esperaba Money, fue {other:?}"),
        }
    }

    #[test]
    fn max_tokens_cubre_el_presupuesto_de_salida_estimado() {
        // El tope de salida debe ser >= lo que el estimate proyecta por idea, más
        // un margen: si fuera menor, la respuesta se truncaría y se cobraría por
        // un JSON inválido (auditoría P2).
        for n in [0usize, 1, 13, 50, 200] {
            let budget = OUTPUT_TOKENS_PER_IDEA * n as u64;
            let max = max_output_tokens_for(n);
            assert!(
                max >= budget,
                "n={n}: max_tokens {max} < presupuesto {budget}"
            );
        }
        // Caso del informe: 13 ideas (13*80=1040) requiere más que el viejo 1024.
        assert!(max_output_tokens_for(13) > 1024);
        // Piso: pocas ideas igual dejan lugar para un array razonable.
        assert!(max_output_tokens_for(0) >= 512);
    }

    #[test]
    fn gemini_es_mas_barato_que_anthropic() {
        let ideas = [idea("signals"), idea("rxjs")];
        let a = match estimate_ideas_ai(AiProvider::Anthropic, &ideas).kind {
            CostKind::Money { usd_micros } => usd_micros,
            _ => unreachable!(),
        };
        let g = match estimate_ideas_ai(AiProvider::Gemini, &ideas).kind {
            CostKind::Money { usd_micros } => usd_micros,
            _ => unreachable!(),
        };
        assert!(g < a, "Gemini Flash debería estimar más barato que Opus");
    }
}
