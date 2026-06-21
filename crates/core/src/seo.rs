//! Auditoría SEO heurística de título/tags/descripción (F8). **Determinista y sin
//! red**: revisa largos recomendados, stuffing, tags vacíos/duplicados y —lo más
//! valioso— cruza con las keywords que **demanda la comunidad** (F6) para detectar
//! oportunidades que el creador no está aprovechando.
//!
//! Dominio puro: la metadata real (F9) y la sugerencia con IA (F12) se enchufan
//! por encima; acá solo el juicio heurístico sobre el texto candidato.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::text::{content_tokens, normalize_text, CorpusInsights};

// Largos recomendados (caracteres). Heurística de SEO de YouTube.
const TITLE_MIN: usize = 30;
const TITLE_MAX: usize = 70;
const DESC_MIN: usize = 100;
const MIN_TAGS: usize = 3;
/// Cuántas keywords top de la comunidad se cruzan contra el texto candidato.
const TOP_COMMUNITY: usize = 10;
/// Repeticiones de una misma palabra de contenido en el título que cuentan como
/// stuffing.
const STUFFING_REPEATS: u32 = 3;

/// El texto candidato que pega el usuario (o que viene de un `VideoMeta`, F9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeoInput {
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub description: String,
}

/// Gravedad de un hallazgo, de mayor a menor impacto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeoSeverity {
    /// Bloqueante: falta algo esencial (título/descripción vacíos).
    Critical,
    /// Conviene corregir (largos fuera de rango, stuffing, duplicados).
    Warning,
    /// Sugerencia menor / oportunidad.
    Info,
}

impl SeoSeverity {
    fn rank(self) -> u8 {
        match self {
            SeoSeverity::Critical => 0,
            SeoSeverity::Warning => 1,
            SeoSeverity::Info => 2,
        }
    }

    fn penalty(self) -> u32 {
        match self {
            SeoSeverity::Critical => 25,
            SeoSeverity::Warning => 10,
            SeoSeverity::Info => 4,
        }
    }
}

/// Un hallazgo concreto de la auditoría.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeoFinding {
    pub severity: SeoSeverity,
    /// Área afectada: `"título"`, `"tags"`, `"descripción"`, `"keywords"`.
    pub area: String,
    pub message: String,
}

/// Reporte completo: puntaje 0–100, hallazgos ordenados por gravedad, y las
/// keywords demandadas por la comunidad que el texto NO aprovecha.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeoReport {
    pub score: u8,
    pub findings: Vec<SeoFinding>,
    pub missing_community_keywords: Vec<String>,
}

fn finding(severity: SeoSeverity, area: &str, message: impl Into<String>) -> SeoFinding {
    SeoFinding {
        severity,
        area: area.into(),
        message: message.into(),
    }
}

/// Audita el título: vacío, largo fuera de rango y keyword stuffing.
pub fn audit_title(title: &str) -> Vec<SeoFinding> {
    let mut out = Vec::new();
    let len = title.trim().chars().count();
    if len == 0 {
        out.push(finding(
            SeoSeverity::Critical,
            "título",
            "El título está vacío.",
        ));
        return out;
    }
    if len < TITLE_MIN {
        out.push(finding(
            SeoSeverity::Warning,
            "título",
            format!("Título corto ({len} caracteres): apuntá a {TITLE_MIN}-{TITLE_MAX}."),
        ));
    } else if len > TITLE_MAX {
        out.push(finding(
            SeoSeverity::Warning,
            "título",
            format!("Título largo ({len}): puede cortarse en los resultados; ideal {TITLE_MIN}-{TITLE_MAX}."),
        ));
    }
    // Stuffing: una palabra de contenido repetida muchas veces.
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for t in content_tokens(title) {
        *counts.entry(t).or_insert(0) += 1;
    }
    if counts.values().any(|&n| n >= STUFFING_REPEATS) {
        out.push(finding(
            SeoSeverity::Warning,
            "título",
            "Parece keyword stuffing: una palabra se repite demasiado en el título.",
        ));
    }
    out
}

/// Audita los tags: ausencia, pocos, vacíos y duplicados (normalizados).
pub fn audit_tags(tags: &[String]) -> Vec<SeoFinding> {
    let mut out = Vec::new();
    let nonempty: Vec<&String> = tags.iter().filter(|t| !t.trim().is_empty()).collect();

    if nonempty.is_empty() {
        out.push(finding(
            SeoSeverity::Warning,
            "tags",
            "No hay tags: agregá algunos para mejorar el alcance.",
        ));
        return out;
    }
    if nonempty.len() < MIN_TAGS {
        out.push(finding(
            SeoSeverity::Info,
            "tags",
            format!(
                "Pocos tags ({}): sumá hasta {MIN_TAGS} o más.",
                nonempty.len()
            ),
        ));
    }
    if tags.iter().any(|t| t.trim().is_empty()) {
        out.push(finding(SeoSeverity::Info, "tags", "Hay tags vacíos."));
    }
    // Duplicados (case-insensitive / sin acentos).
    let mut seen: HashSet<String> = HashSet::new();
    let mut has_dups = false;
    for t in &nonempty {
        let key = normalize_text(t).join(" ");
        if !seen.insert(key) {
            has_dups = true;
        }
    }
    if has_dups {
        out.push(finding(
            SeoSeverity::Warning,
            "tags",
            "Hay tags duplicados.",
        ));
    }
    out
}

/// Audita la descripción: vacía o demasiado corta.
pub fn audit_description(description: &str) -> Vec<SeoFinding> {
    let mut out = Vec::new();
    let len = description.trim().chars().count();
    if len == 0 {
        out.push(finding(
            SeoSeverity::Critical,
            "descripción",
            "La descripción está vacía.",
        ));
    } else if len < DESC_MIN {
        out.push(finding(
            SeoSeverity::Warning,
            "descripción",
            format!("Descripción corta ({len}): apuntá a {DESC_MIN}+ caracteres."),
        ));
    }
    out
}

/// Auditoría completa: combina las reglas y cruza con las keywords demandadas por
/// la comunidad (F6). Devuelve puntaje 0–100 y hallazgos ordenados por gravedad.
pub fn audit_seo(input: &SeoInput, corpus: &CorpusInsights) -> SeoReport {
    let mut findings = Vec::new();
    findings.extend(audit_title(&input.title));
    findings.extend(audit_tags(&input.tags));
    findings.extend(audit_description(&input.description));

    // Tokens presentes en el texto candidato (título + tags + descripción).
    let mut present: HashSet<String> = HashSet::new();
    present.extend(normalize_text(&input.title));
    present.extend(normalize_text(&input.description));
    for tag in &input.tags {
        present.extend(normalize_text(tag));
    }

    // Keywords top de la comunidad ausentes del texto = oportunidad SEO.
    let missing_community_keywords: Vec<String> = corpus
        .keywords
        .iter()
        .take(TOP_COMMUNITY)
        .filter(|k| !present.contains(&k.term))
        .map(|k| k.term.clone())
        .collect();
    if !missing_community_keywords.is_empty() {
        findings.push(finding(
            SeoSeverity::Info,
            "keywords",
            format!(
                "Tu comunidad pide temas que no figuran acá: {}.",
                missing_community_keywords.join(", ")
            ),
        ));
    }

    // Orden estable por gravedad (Critical primero); dentro, el orden de inserción.
    findings.sort_by_key(|f| f.severity.rank());

    let penalty: u32 = findings.iter().map(|f| f.severity.penalty()).sum();
    let score = 100u32.saturating_sub(penalty) as u8;

    SeoReport {
        score,
        findings,
        missing_community_keywords,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::corpus_insights;

    fn docs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn titulo_vacio_es_critico_y_corta() {
        let f = audit_title("   ");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, SeoSeverity::Critical);
    }

    #[test]
    fn titulo_corto_y_largo_son_warning() {
        assert_eq!(audit_title("Angular").len(), 1); // corto
        assert_eq!(audit_title("Angular")[0].severity, SeoSeverity::Warning);
        let largo = "Tutorial ultra completo de Angular signals routing forms testing deploy y mucho mas todavia";
        assert!(audit_title(largo)
            .iter()
            .any(|f| f.message.contains("largo")));
    }

    #[test]
    fn titulo_en_rango_no_tiene_hallazgos() {
        // 30-70 chars, sin repeticiones.
        let ok = "Guia practica de Angular signals hoy";
        assert!(
            audit_title(ok).is_empty(),
            "título en rango no debe tener hallazgos"
        );
    }

    #[test]
    fn detecta_keyword_stuffing_en_titulo() {
        let f = audit_title("signals signals signals en angular para todos hoy");
        assert!(f.iter().any(|x| x.message.contains("stuffing")));
    }

    #[test]
    fn tags_vacios_pocos_y_duplicados() {
        assert!(audit_tags(&[])
            .iter()
            .any(|f| f.severity == SeoSeverity::Warning));
        // pocos (1) -> Info
        assert!(audit_tags(&["angular".into()])
            .iter()
            .any(|f| f.severity == SeoSeverity::Info));
        // duplicado (case/acentos) -> Warning
        let dup = vec!["Angular".into(), "angular".into(), "signals".into()];
        assert!(audit_tags(&dup)
            .iter()
            .any(|f| f.message.contains("duplicados")));
        // tag vacío entre medio -> Info "vacíos"
        let empty_mixed = vec!["angular".into(), "".into(), "signals".into(), "rxjs".into()];
        assert!(audit_tags(&empty_mixed)
            .iter()
            .any(|f| f.message.contains("vacíos")));
    }

    #[test]
    fn descripcion_vacia_y_corta() {
        assert_eq!(audit_description("")[0].severity, SeoSeverity::Critical);
        assert_eq!(audit_description("Breve")[0].severity, SeoSeverity::Warning);
        let larga = "Esta descripción es deliberadamente larga para superar el mínimo recomendado \
            de cien caracteres y así no disparar el hallazgo de descripción corta en la auditoría.";
        assert!(audit_description(larga).is_empty());
    }

    #[test]
    fn audit_seo_cruza_keywords_de_la_comunidad() {
        // Corpus: la comunidad habla mucho de "signals" y "routing".
        let corpus = corpus_insights(&docs(&[
            "signals signals routing",
            "mas signals y routing",
            "routing avanzado",
            "hablemos de signals",
        ]));
        // El input no menciona esas keywords.
        let input = SeoInput {
            title: "Un video sobre cualquier otra cosa distinta hoy".into(),
            tags: vec!["varios".into(), "otros".into(), "cosas".into()],
            description: "Una descripción larga que igual no toca los temas que la comunidad \
                viene pidiendo en sus comentarios desde hace bastante tiempo ya."
                .into(),
        };
        let report = audit_seo(&input, &corpus);
        assert!(
            report
                .missing_community_keywords
                .contains(&"signals".to_string()),
            "debe marcar signals como faltante"
        );
        assert!(report.findings.iter().any(|f| f.area == "keywords"));
    }

    #[test]
    fn score_baja_con_hallazgos_y_ordena_por_gravedad() {
        let empty_corpus = corpus_insights(&[]);
        // Título vacío (Critical) + sin tags (Warning) + desc vacía (Critical).
        let bad = SeoInput {
            title: "".into(),
            tags: vec![],
            description: "".into(),
        };
        let report = audit_seo(&bad, &empty_corpus);
        assert!(
            report.score < 60,
            "muchos problemas -> score bajo, fue {}",
            report.score
        );
        // El primer hallazgo es de gravedad Critical (orden por gravedad).
        assert_eq!(report.findings[0].severity, SeoSeverity::Critical);
    }

    #[test]
    fn input_impecable_da_score_alto_y_determinista() {
        let empty_corpus = corpus_insights(&[]);
        let good = SeoInput {
            title: "Guia practica de Angular signals para principiantes".into(),
            tags: vec![
                "angular".into(),
                "signals".into(),
                "tutorial".into(),
                "frontend".into(),
            ],
            description: "En este video recorremos signals de Angular paso a paso, con ejemplos \
                reales y buenas practicas para que puedas aplicarlo en tu proyecto hoy mismo."
                .into(),
        };
        let report = audit_seo(&good, &empty_corpus);
        assert_eq!(
            report.score, 100,
            "sin hallazgos -> 100, fue {}",
            report.score
        );
        assert!(report.findings.is_empty());
        // Determinismo.
        assert_eq!(report, audit_seo(&good, &empty_corpus));
    }
}
