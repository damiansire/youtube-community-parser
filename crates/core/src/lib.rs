//! `sdp-core` — dominio puro del sistema de trackeo de comentarios.
//!
//! Sin dependencias de Tauri, red ni base de datos: solo modelos y la lógica
//! para responder *quiénes comentan más y quiénes menos*. Se testea sin UI.

pub mod cost;
pub mod ideas;
pub mod models;
pub mod ranking;
pub mod seo;
pub mod text;

pub use cost::{
    estimate_search, estimate_video_meta, needs_optin, CostEstimate, CostKind, CostLine, CostPolicy,
};
pub use ideas::{detect_questions, detect_requests, mine_video_ideas, DemandSignal, VideoIdea};
pub use models::{ChannelId, Comment, Commenter, SearchHit, SearchPlan, VideoMeta};
pub use ranking::{
    least_active, least_active_of, most_active, most_active_of, rank_commenters, CommenterStats,
};
pub use seo::{
    audit_description, audit_seo, audit_tags, audit_title, SeoFinding, SeoInput, SeoReport,
    SeoSeverity,
};
pub use text::{
    cluster_topics, corpus_insights, extract_keywords, CorpusInsights, KeywordStats, Topic,
};
