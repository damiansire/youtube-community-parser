//! Ideas de video desde la comunidad (F7). **Heurístico y determinista**: mina
//! los comentarios ya persistidos buscando *demanda* —preguntas, pedidos y temas
//! recurrentes— y la convierte en semillas de ideas, con citas que la respaldan.
//!
//! Reusa F6 ([`crate::text`]): agrupa por la keyword dominante de cada comentario
//! y puntúa por frecuencia + likes. Dominio puro: sin red, sin IA (la capa de IA
//! opcional llega en F12, detrás del mismo `VideoIdea`).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::models::Comment;
use crate::text::content_tokens;

/// Citas de muestra que acompañan a cada idea.
const SAMPLE_QUOTES: usize = 2;

/// Qué tipo de demanda originó la idea.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DemandSignal {
    /// La comunidad pregunta algo (cómo, por qué, …).
    Question,
    /// La comunidad pide explícitamente un video/tutorial.
    Request,
    /// Un tema que se repite, sin pregunta ni pedido explícito.
    RecurringTopic,
}

/// Una idea de video derivada de la comunidad.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoIdea {
    /// Semilla del título (la keyword/tema dominante que la origina).
    pub title_seed: String,
    /// Señal de demanda más fuerte presente en el grupo.
    pub signal: DemandSignal,
    /// IDs de los comentarios que respaldan la idea (ordenados, deterministas).
    pub supporting_comment_ids: Vec<String>,
    /// Puntaje: frecuencia + likes (Σ de `1 + like_count` sobre los comentarios).
    pub score: u64,
    /// Citas de muestra (los comentarios más likeados del grupo, escapables en UI).
    pub sample_quotes: Vec<String>,
}

/// ¿El texto es una pregunta? Heurístico ES+EN: signo de interrogación o un
/// interrogativo al arranque. PURO.
pub fn detect_questions(text: &str) -> bool {
    if text.contains('?') || text.contains('¿') {
        return true;
    }
    const INTERROGATIVES: &[&str] = &[
        "como", "que", "porque", "cuando", "donde", "cual", "cuales", "quien", "how", "what",
        "why", "when", "where", "which", "who",
    ];
    // Un interrogativo entre los primeros tokens (evita falsos positivos de "que"
    // en medio de una oración afirmativa).
    crate::text::normalize_text(text)
        .iter()
        .take(2)
        .any(|t| INTERROGATIVES.contains(&t.as_str()))
}

/// ¿El texto pide explícitamente un video/tutorial? Heurístico ES+EN por frases.
/// PURO. (Se normaliza primero: minúsculas, sin acentos.)
pub fn detect_requests(text: &str) -> bool {
    let normalized = crate::text::normalize_text(text).join(" ");
    const PHRASES: &[&str] = &[
        // español
        "podrias",
        "podes hacer",
        "puedes hacer",
        "hace un video",
        "haga un video",
        "haz un video",
        "me gustaria",
        "quiero ver",
        "seria bueno",
        "estaria bueno",
        "tutorial de",
        "tutorial sobre",
        "video sobre",
        "video de",
        "explica",
        "necesito un",
        // inglés
        "please make",
        "can you make",
        "could you",
        "do a video",
        "make a video",
        "please do",
        "tutorial on",
        "video about",
        "i want",
    ];
    PHRASES.iter().any(|p| normalized.contains(p))
}

/// Mina ideas de video desde los comentarios (F7). Agrupa cada comentario por su
/// **tema dominante** —la keyword de contenido (F6) que más se repite en el
/// corpus (frecuencia de documento)— clasifica la señal de demanda del grupo y
/// puntúa por frecuencia + likes. Determinista; corpus sin keywords → vacío.
///
/// Se usa frecuencia de documento (no tf-idf) a propósito: para agrupar por *tema
/// recurrente* interesa lo que más se comparte, no lo más distintivo.
pub fn mine_video_ideas(comments: &[Comment]) -> Vec<VideoIdea> {
    if comments.is_empty() {
        return Vec::new();
    }

    // Tokens de contenido por comentario (F6) y en cuántos comentarios aparece
    // cada término (frecuencia de documento).
    let per_comment: Vec<Vec<String>> = comments.iter().map(|c| content_tokens(&c.text)).collect();
    let mut doc_freq: HashMap<&str, u64> = HashMap::new();
    for tokens in &per_comment {
        let mut seen: HashSet<&str> = HashSet::new();
        for t in tokens {
            if seen.insert(t.as_str()) {
                *doc_freq.entry(t.as_str()).or_insert(0) += 1;
            }
        }
    }

    // Cada comentario cae en a lo sumo un grupo: el de su tema más recurrente
    // (mayor frecuencia de documento; desempate alfabético, determinista).
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, tokens) in per_comment.iter().enumerate() {
        let dominant = tokens
            .iter()
            .max_by(|a, b| {
                doc_freq[a.as_str()]
                    .cmp(&doc_freq[b.as_str()])
                    .then(b.as_str().cmp(a.as_str())) // empate: gana el término alfabéticamente menor
            })
            .cloned();
        if let Some(keyword) = dominant {
            groups.entry(keyword).or_default().push(i);
        }
    }

    let mut ideas: Vec<VideoIdea> = groups
        .into_iter()
        .map(|(title_seed, idxs)| build_idea(title_seed, &idxs, comments))
        .collect();

    // Más demandadas primero; desempate alfabético por la semilla (determinista).
    ideas.sort_by(|a, b| b.score.cmp(&a.score).then(a.title_seed.cmp(&b.title_seed)));
    ideas
}

/// Arma una idea a partir de un grupo de comentarios (índices) que comparten
/// keyword dominante.
fn build_idea(title_seed: String, idxs: &[usize], comments: &[Comment]) -> VideoIdea {
    // Señal: pregunta manda sobre pedido, y pedido sobre tema recurrente.
    let any_question = idxs.iter().any(|&i| detect_questions(&comments[i].text));
    let any_request = idxs.iter().any(|&i| detect_requests(&comments[i].text));
    let signal = if any_question {
        DemandSignal::Question
    } else if any_request {
        DemandSignal::Request
    } else {
        DemandSignal::RecurringTopic
    };

    let score: u64 = idxs.iter().map(|&i| 1 + comments[i].like_count).sum();

    let mut supporting_comment_ids: Vec<String> =
        idxs.iter().map(|&i| comments[i].id.clone()).collect();
    supporting_comment_ids.sort();

    // Citas: los comentarios más likeados del grupo (desempate por id, estable).
    let mut by_likes = idxs.to_vec();
    by_likes.sort_by(|&a, &b| {
        comments[b]
            .like_count
            .cmp(&comments[a].like_count)
            .then(comments[a].id.cmp(&comments[b].id))
    });
    let sample_quotes: Vec<String> = by_likes
        .iter()
        .take(SAMPLE_QUOTES)
        .map(|&i| comments[i].text.clone())
        .collect();

    VideoIdea {
        title_seed,
        signal,
        supporting_comment_ids,
        score,
        sample_quotes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn comment(id: &str, text: &str, likes: u64) -> Comment {
        Comment {
            id: id.into(),
            video_id: "v1".into(),
            author_channel_id: "ana".into(),
            text: text.into(),
            like_count: likes,
            published_at: Utc.with_ymd_and_hms(2021, 9, 27, 3, 0, 0).single().unwrap(),
        }
    }

    #[test]
    fn detecta_preguntas_es_y_en() {
        assert!(detect_questions("¿Cómo configuro signals?"));
        assert!(detect_questions("How do I deploy this?"));
        assert!(detect_questions("Por que falla el routing")); // interrogativo al inicio
        assert!(!detect_questions("Buen video, muy claro"));
        assert!(!detect_questions("Me encantó el tutorial")); // "que" no está al inicio
    }

    #[test]
    fn detecta_pedidos_es_y_en() {
        assert!(detect_requests("¿Podrías hacer un video de signals?"));
        assert!(detect_requests("please make a tutorial on rxjs"));
        assert!(detect_requests("estaría bueno un tutorial de testing"));
        assert!(!detect_requests("gracias por el contenido"));
    }

    #[test]
    fn mina_ideas_agrupando_por_tema_y_clasifica_senal() {
        let comments = vec![
            comment("c1", "¿Cómo uso signals en angular?", 2),
            comment("c2", "podrias hacer un video de signals porfa", 5),
            comment("c3", "signals signals signals me encantan", 1),
            comment("c4", "hablemos de cocina", 0),
        ];
        let ideas = mine_video_ideas(&comments);
        // "signals" es la keyword dominante de c1..c3 -> una sola idea.
        let signals = ideas
            .iter()
            .find(|i| i.title_seed == "signals")
            .expect("debe haber una idea de signals");
        assert_eq!(signals.supporting_comment_ids, vec!["c1", "c2", "c3"]);
        // Hay una pregunta (c1) -> la señal es Question (manda sobre Request).
        assert_eq!(signals.signal, DemandSignal::Question);
        // score = (1+2)+(1+5)+(1+1) = 11.
        assert_eq!(signals.score, 11);
    }

    #[test]
    fn senal_request_cuando_hay_pedido_sin_pregunta() {
        let comments = vec![
            comment("c1", "estaria bueno un tutorial de testing", 3),
            comment("c2", "testing testing siempre testing", 0),
        ];
        let ideas = mine_video_ideas(&comments);
        let idea = ideas.iter().find(|i| i.title_seed == "testing").unwrap();
        assert_eq!(idea.signal, DemandSignal::Request);
    }

    #[test]
    fn senal_recurring_topic_sin_pregunta_ni_pedido() {
        let comments = vec![
            comment("c1", "deploy deploy deploy", 0),
            comment("c2", "siempre hablando de deploy", 1),
        ];
        let ideas = mine_video_ideas(&comments);
        let idea = ideas.iter().find(|i| i.title_seed == "deploy").unwrap();
        assert_eq!(idea.signal, DemandSignal::RecurringTopic);
    }

    #[test]
    fn score_respeta_likes_y_frecuencia_y_ordena() {
        let comments = vec![
            // tema "alfa": 1 comentario con muchos likes
            comment("a1", "alfa alfa", 50),
            // tema "beta": 2 comentarios con pocos likes
            comment("b1", "beta beta", 1),
            comment("b2", "beta otra vez", 1),
        ];
        let ideas = mine_video_ideas(&comments);
        // alfa: 1+50 = 51 ; beta: (1+1)+(1+1)=4 -> alfa primero.
        assert_eq!(ideas[0].title_seed, "alfa");
        assert_eq!(ideas[0].score, 51);
    }

    #[test]
    fn citas_de_muestra_priorizan_likes_y_se_topean() {
        let comments = vec![
            comment("c1", "signals poco likeado", 0),
            comment("c2", "signals muy likeado", 99),
            comment("c3", "signals medio", 10),
        ];
        let ideas = mine_video_ideas(&comments);
        let idea = &ideas[0];
        assert_eq!(idea.sample_quotes.len(), 2, "tope de 2 citas");
        assert_eq!(idea.sample_quotes[0], "signals muy likeado"); // más likes primero
    }

    #[test]
    fn determinismo_y_corpus_vacio() {
        assert!(mine_video_ideas(&[]).is_empty());
        let comments = vec![
            comment("c1", "¿como uso signals?", 2),
            comment("c2", "tutorial de signals porfa", 3),
        ];
        assert_eq!(mine_video_ideas(&comments), mine_video_ideas(&comments));
        // comentarios sin keyword de contenido (puras stopwords) -> sin ideas.
        assert!(mine_video_ideas(&[comment("x", "de la y el que", 0)]).is_empty());
    }
}
