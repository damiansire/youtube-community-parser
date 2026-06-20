//! `sdp-core` — dominio puro del sistema de trackeo de comentarios.
//!
//! Sin dependencias de Tauri, red ni base de datos: solo modelos y la lógica
//! para responder *quiénes comentan más y quiénes menos*. Se testea sin UI.

pub mod models;
pub mod ranking;

pub use models::{ChannelId, Comment, Commenter};
pub use ranking::{least_active, most_active, rank_commenters, CommenterStats};
