//! Minería de texto del corpus de comentarios (F6): keywords y temas
//! recurrentes. **Heurístico y determinista** — frecuencia + peso tipo tf-idf y
//! co-ocurrencia de bigramas; nada de ML, nada de red. Es el cimiento de F7
//! (ideas) y F8 (SEO).
//!
//! Todo es **dominio puro**: opera sobre los textos ya persistidos y el orden de
//! salida es estable (mismo desempate determinista que `rank_commenters`), para
//! que sea testeable con fixtures inline.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Largo mínimo de un token para considerarlo keyword (descarta "ok", "jaja"
/// cortos y ruido de un solo carácter).
const MIN_TOKEN_LEN: usize = 3;

/// Cantidad de keywords/temas que devuelve [`corpus_insights`] por defecto.
pub const DEFAULT_KEYWORD_LIMIT: usize = 50;
pub const DEFAULT_TOPIC_LIMIT: usize = 20;

/// Estadística de una keyword dentro del corpus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeywordStats {
    /// Término normalizado (minúsculas, sin acentos).
    pub term: String,
    /// Apariciones totales en el corpus.
    pub total_count: u64,
    /// En cuántos comentarios (documentos) aparece.
    pub document_count: u64,
    /// Peso tipo tf-idf: `total_count · ln(N / document_count)`. Baja a 0 los
    /// términos ubicuos (presentes en todos los documentos).
    pub weight: f64,
}

/// Un tema recurrente: un par de términos de contenido que co-ocurren (bigrama
/// adyacente) en varios comentarios.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topic {
    /// Los términos que forman el tema (par adyacente, en orden de aparición).
    pub terms: Vec<String>,
    /// En cuántos comentarios aparece el bigrama.
    pub document_count: u64,
}

/// Resultado de analizar el corpus: keywords demandadas y temas recurrentes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorpusInsights {
    pub keywords: Vec<KeywordStats>,
    pub topics: Vec<Topic>,
    /// Cantidad de comentarios analizados.
    pub document_count: u64,
}

/// Lista embebida de stopwords ES + EN (palabras vacías que no aportan tema).
pub fn stopwords_es_en() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| STOPWORDS.iter().copied().collect())
}

/// ¿Es `token` una stopword? (Se asume ya normalizado: minúsculas/sin acentos.)
pub fn is_stopword(token: &str) -> bool {
    stopwords_es_en().contains(token)
}

/// Normaliza un texto a tokens: minúsculas, sin acentos en vocales, partido por
/// todo lo que no sea alfanumérico. PURO y determinista. No filtra stopwords ni
/// largo (eso lo hace cada función según su criterio).
pub fn normalize_text(text: &str) -> Vec<String> {
    let folded: String = text.chars().flat_map(fold_lower).collect();
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Tokens "de contenido": normalizados, sin stopwords y con largo mínimo. Es la
/// base tanto de keywords como de temas (y del minado de ideas en F7).
pub fn content_tokens(text: &str) -> Vec<String> {
    normalize_text(text)
        .into_iter()
        .filter(|t| t.chars().count() >= MIN_TOKEN_LEN && !is_stopword(t))
        .collect()
}

/// Pasa un carácter a minúscula y le saca el acento a las vocales (á→a, ü→u),
/// conservando la ñ. Devuelve un iterador porque `to_lowercase` puede expandir.
fn fold_lower(c: char) -> impl Iterator<Item = char> {
    c.to_lowercase().map(|lc| match lc {
        'á' | 'à' | 'ä' | 'â' => 'a',
        'é' | 'è' | 'ë' | 'ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' => 'o',
        'ú' | 'ù' | 'ü' | 'û' => 'u',
        other => other,
    })
}

/// Extrae las keywords del corpus, ordenadas por peso tf-idf (desc), con
/// desempate determinista por frecuencia total y luego alfabético. Devuelve a lo
/// sumo `limit`. Corpus vacío → vacío.
pub fn extract_keywords(documents: &[String], limit: usize) -> Vec<KeywordStats> {
    let n_docs = documents.len() as f64;
    let mut total: HashMap<String, u64> = HashMap::new();
    let mut docs: HashMap<String, u64> = HashMap::new();

    for doc in documents {
        let tokens = content_tokens(doc);
        let mut seen_in_doc: HashSet<&str> = HashSet::new();
        for t in &tokens {
            *total.entry(t.clone()).or_insert(0) += 1;
            if seen_in_doc.insert(t.as_str()) {
                *docs.entry(t.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut keywords: Vec<KeywordStats> = total
        .into_iter()
        .map(|(term, total_count)| {
            let document_count = docs.get(&term).copied().unwrap_or(0);
            // idf = ln(N / df): 0 cuando el término está en todos los documentos.
            let idf = (n_docs / document_count as f64).ln();
            KeywordStats {
                weight: total_count as f64 * idf,
                term,
                total_count,
                document_count,
            }
        })
        .collect();

    keywords.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.total_count.cmp(&a.total_count))
            .then(a.term.cmp(&b.term))
    });
    keywords.truncate(limit);
    keywords
}

/// Agrupa temas recurrentes como bigramas adyacentes de tokens de contenido,
/// ordenados por en cuántos comentarios aparecen (desc) y desempate alfabético.
/// Determinista, sin ML. Devuelve a lo sumo `limit`.
pub fn cluster_topics(documents: &[String], limit: usize) -> Vec<Topic> {
    // clave: el par de términos; valor: en cuántos documentos aparece.
    let mut bigram_docs: HashMap<(String, String), u64> = HashMap::new();

    for doc in documents {
        let tokens = content_tokens(doc);
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for pair in tokens.windows(2) {
            // ignora bigramas de una palabra repetida ("muy muy").
            if pair[0] == pair[1] {
                continue;
            }
            let key = (pair[0].clone(), pair[1].clone());
            if seen.insert(key.clone()) {
                *bigram_docs.entry(key).or_insert(0) += 1;
            }
        }
    }

    let mut topics: Vec<Topic> = bigram_docs
        .into_iter()
        .map(|((a, b), document_count)| Topic {
            terms: vec![a, b],
            document_count,
        })
        .collect();

    topics.sort_by(|x, y| {
        y.document_count
            .cmp(&x.document_count)
            .then(x.terms.cmp(&y.terms))
    });
    topics.truncate(limit);
    topics
}

/// Analiza el corpus completo: keywords + temas + cantidad de documentos.
/// Conveniencia con límites por defecto sobre [`extract_keywords`] y
/// [`cluster_topics`].
pub fn corpus_insights(documents: &[String]) -> CorpusInsights {
    CorpusInsights {
        keywords: extract_keywords(documents, DEFAULT_KEYWORD_LIMIT),
        topics: cluster_topics(documents, DEFAULT_TOPIC_LIMIT),
        document_count: documents.len() as u64,
    }
}

/// Stopwords ES + EN embebidas (curado pragmático, no exhaustivo).
const STOPWORDS: &[&str] = &[
    // --- español ---
    "de", "la", "que", "el", "en", "los", "del", "las", "por", "para", "con", "una", "como", "mas",
    "pero", "sus", "este", "esta", "entre", "cuando", "muy", "sin", "sobre", "tambien", "hasta",
    "hay", "donde", "quien", "desde", "todo", "todos", "uno", "les", "contra", "otros", "ese",
    "eso", "ante", "ellos", "esto", "antes", "algunos", "unos", "otro", "otras", "otra", "tanto",
    "esa", "estos", "mucho", "quienes", "nada", "muchos", "cual", "poco", "ella", "estar", "estas",
    "algunas", "algo", "nosotros", "mis", "tus", "ellas", "esto", "son", "fue", "ser", "porque",
    "asi", "aca", "alla", "cada", "ya", "solo", "tiene", "tienen", "hace", "hacer", "puede", "ver",
    "bien", "vez", "asi", "gran", "decir", "dijo", "va", "van", // --- inglés ---
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "her", "was", "one",
    "our", "out", "day", "had", "has", "his", "how", "man", "new", "now", "old", "see", "two",
    "way", "who", "did", "its", "let", "put", "say", "she", "too", "use", "that", "this", "with",
    "they", "from", "your", "have", "more", "will", "about", "would", "there", "their", "what",
    "when", "which", "them", "then", "than", "into", "just", "like", "some", "such", "only",
    "very", "also", "been", "were", "here",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn normaliza_minuscula_sin_acentos_y_tokeniza() {
        // Saca acentos de vocales (Cómo->como) pero CONSERVA la ñ (año != ano).
        let tokens = normalize_text("¡Señales en Angular! ¿Cómo? video-2");
        assert_eq!(
            tokens,
            vec!["señales", "en", "angular", "como", "video", "2"]
        );
    }

    #[test]
    fn stopwords_filtran_palabras_vacias_es_y_en() {
        assert!(is_stopword("de"));
        assert!(is_stopword("the"));
        assert!(!is_stopword("angular"));
        assert!(!is_stopword("señales")); // sin normalizar no matchea; ok, se normaliza antes
    }

    #[test]
    fn extrae_termino_dominante() {
        // "angular" domina por frecuencia (5) sin ser ubicuo (3/4 docs): tf-idf
        // lo deja arriba de términos que aparecen una sola vez.
        let corpus = docs(&[
            "angular angular tutorial",
            "angular tips angular",
            "angular guide",
            "cocina receta hoy",
        ]);
        let kws = extract_keywords(&corpus, 10);
        assert_eq!(kws[0].term, "angular");
        assert_eq!(kws[0].document_count, 3);
        assert_eq!(kws[0].total_count, 5);
    }

    #[test]
    fn tfidf_baja_los_terminos_ubicuos() {
        // "angular" en TODOS los documentos -> idf 0 -> weight 0 (no domina).
        // "signals" en pocos -> weight > 0 -> rankea por encima.
        let corpus = docs(&[
            "angular signals",
            "angular routing",
            "angular forms",
            "angular cli",
        ]);
        let kws = extract_keywords(&corpus, 10);
        let angular = kws.iter().find(|k| k.term == "angular").unwrap();
        assert_eq!(angular.weight, 0.0, "término ubicuo debe tener peso 0");
        // el primero no puede ser el ubicuo.
        assert_ne!(kws[0].term, "angular");
    }

    #[test]
    fn keywords_orden_es_determinista() {
        let corpus = docs(&["alfa beta", "beta alfa", "alfa beta"]);
        let a = extract_keywords(&corpus, 10);
        let b = extract_keywords(&corpus, 10);
        assert_eq!(a, b);
    }

    #[test]
    fn agrupa_temas_por_coocurrencia() {
        let corpus = docs(&[
            "tutorial angular signals",
            "mas angular signals porfa",
            "angular signals rocks",
            "react hooks tutorial",
        ]);
        let topics = cluster_topics(&corpus, 10);
        // "angular signals" co-ocurre en 3 documentos -> tema top.
        assert_eq!(topics[0].terms, vec!["angular", "signals"]);
        assert_eq!(topics[0].document_count, 3);
    }

    #[test]
    fn corpus_vacio_da_insights_vacios() {
        let empty = corpus_insights(&[]);
        assert!(empty.keywords.is_empty());
        assert!(empty.topics.is_empty());
        assert_eq!(empty.document_count, 0);

        // un corpus de puras stopwords/cortas tampoco produce keywords.
        let noise = corpus_insights(&docs(&["de la y el", "the and a"]));
        assert!(noise.keywords.is_empty());
        assert!(noise.topics.is_empty());
        assert_eq!(noise.document_count, 2);
    }

    #[test]
    fn respeta_el_limite() {
        let corpus = docs(&["uno dos tres cuatro cinco seis siete ocho"]);
        assert_eq!(extract_keywords(&corpus, 3).len(), 3);
    }
}
