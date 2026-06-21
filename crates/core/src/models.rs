//! Modelos de dominio.
//!
//! Reflejan la forma de los datos que devuelve la YouTube Data API v3
//! (ver el sidecar `ingest`), pero desacoplados de la fuente: el dominio
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

/// Metadata de un video propio del creador (F9). Es el dato de dominio que
/// alimenta el SEO real (título/tags/descripción) y el benchmark (engagement),
/// desacoplado de la forma de `videos.list`: el mapeo desde la API vive en el
/// boundary (`youtube.rs`), acá solo el modelo puro.
///
/// Las cuentas (`view_count`, etc.) son `Option` porque YouTube las omite cuando
/// el creador ocultó las estadísticas del video; el dominio distingue "0" de
/// "no informado" sin inventar ceros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoMeta {
    /// `id` del video en YouTube.
    pub video_id: String,
    /// Canal dueño del video (`snippet.channelId`).
    pub channel_id: ChannelId,
    /// `snippet.title`.
    pub title: String,
    /// `snippet.description`.
    #[serde(default)]
    pub description: String,
    /// `snippet.tags` (puede venir vacío o ausente).
    #[serde(default)]
    pub tags: Vec<String>,
    /// `statistics.viewCount` (None si el video oculta estadísticas).
    #[serde(default)]
    pub view_count: Option<u64>,
    /// `statistics.likeCount` (None si está oculto/deshabilitado).
    #[serde(default)]
    pub like_count: Option<u64>,
    /// `statistics.commentCount` (None si los comentarios están deshabilitados).
    #[serde(default)]
    pub comment_count: Option<u64>,
    /// `snippet.publishedAt`.
    pub published_at: DateTime<Utc>,
}

/// Un resultado de búsqueda/trending (F10), mapeado desde `search.list`. Es el
/// dato mínimo para descubrir videos de la competencia o temas en tendencia;
/// la metadata rica se trae luego con `videos.list` (F9) sobre estos ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    /// `id.videoId`.
    pub video_id: String,
    /// Canal dueño del resultado (`snippet.channelId`).
    pub channel_id: ChannelId,
    /// `snippet.title`.
    pub title: String,
    /// `snippet.publishedAt`.
    pub published_at: DateTime<Utc>,
}

/// Parámetros de una búsqueda (F10). `search.list` cuesta **100 unidades por
/// página**, así que `max_pages` es el tope de costo que el usuario confirma en
/// el gate antes de ejecutar (default 1 = 100u).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPlan {
    /// Término de búsqueda (`q`).
    pub query: String,
    /// Si es `true`, ordena por popularidad (descubrir lo que más se ve) en vez
    /// de relevancia textual.
    #[serde(default)]
    pub trending: bool,
    /// Máximo de páginas a pedir (cada una = 100u). Tope de costo.
    pub max_pages: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> VideoMeta {
        VideoMeta {
            video_id: "vid1".into(),
            channel_id: "UCcanal".into(),
            title: "Cómo usar señales en Angular".into(),
            description: "Tutorial completo".into(),
            tags: vec!["angular".into(), "signals".into()],
            view_count: Some(1234),
            like_count: Some(56),
            comment_count: Some(7),
            published_at: "2021-09-27T03:00:00Z".parse().unwrap(),
        }
    }

    #[test]
    fn video_meta_round_trip_serde() {
        let meta = sample_meta();
        let json = serde_json::to_string(&meta).unwrap();
        let back: VideoMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, back);
    }

    #[test]
    fn video_meta_estadisticas_ocultas_son_none_no_cero() {
        // Cuando el video oculta estadísticas, los campos quedan ausentes y el
        // dominio los distingue de un 0 real.
        let json = r#"{
            "video_id": "vid1",
            "channel_id": "UCcanal",
            "title": "t",
            "published_at": "2021-09-27T03:00:00Z"
        }"#;
        let meta: VideoMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.view_count, None);
        assert_eq!(meta.like_count, None);
        assert_eq!(meta.comment_count, None);
        assert!(meta.description.is_empty());
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn search_hit_round_trip_serde() {
        let hit = SearchHit {
            video_id: "v1".into(),
            channel_id: "UCotro".into(),
            title: "Competencia".into(),
            published_at: "2021-09-27T03:00:00Z".parse().unwrap(),
        };
        let json = serde_json::to_string(&hit).unwrap();
        assert_eq!(hit, serde_json::from_str::<SearchHit>(&json).unwrap());
    }

    #[test]
    fn search_plan_trending_default_false() {
        let plan: SearchPlan =
            serde_json::from_str(r#"{ "query": "angular", "max_pages": 1 }"#).unwrap();
        assert!(!plan.trending);
        assert_eq!(plan.max_pages, 1);
    }
}
