//! Modelos de dominio.
//!
//! Reflejan la forma de los datos que devuelve `youtube-fast-api`
//! (ver `getAllComments`), pero desacoplados de la fuente: el dominio
//! no sabe si los comentarios vienen de la API, de un import o de la base.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Identificador de canal de YouTube (el del autor de un comentario o el del
/// canal observado). Es el `authorChannelId` de la API.
pub type ChannelId = String;

/// Una persona que comentó: su identidad de canal en YouTube.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commenter {
    /// `authorChannelId` — clave estable de la persona.
    pub channel_id: ChannelId,
    /// `authorDisplayName`.
    pub display_name: String,
    /// `authorProfileImageUrl`.
    #[serde(default)]
    pub profile_image_url: Option<String>,
    /// `authorChannelUrl`.
    #[serde(default)]
    pub channel_url: Option<String>,
}

/// Un comentario individual sobre un video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// `id` del comentario en YouTube.
    pub id: String,
    /// Video sobre el que se comentó.
    pub video_id: String,
    /// Autor del comentario (`authorChannelId`).
    pub author_channel_id: ChannelId,
    /// Texto mostrado (`textDisplay`).
    pub text: String,
    /// Likes que recibió el comentario.
    #[serde(default)]
    pub like_count: u64,
    /// Cuándo se publicó.
    pub published_at: DateTime<Utc>,
}
