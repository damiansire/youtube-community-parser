//! Cliente nativo de la YouTube Data API v3 (reqwest, async).
//!
//! Reemplaza al sidecar Node (`ingest/`): la app instalada ya no necesita tener
//! `node` ni el árbol de fuentes en una ruta de compile-time, así que es
//! **distribuible** (F1/F10). Todo el efecto de red vive acá; el dominio
//! (`sdp-core`) sigue puro.
//!
//! El parseo es **tipado** (structs `serde`) y la mayor parte —aplanado de un
//! `commentThread`, mapeo a los modelos del core, clasificación de errores de la
//! API— es **pura** (sin red), testeable con fixtures sin API key.
//!
//! Resiliencia de cuota (F4): el recorrido de un canal puede agotar la cuota
//! diaria (10k u/día). Ante un error que NO sea `commentsDisabled` (típicamente
//! `quotaExceeded`) devolvemos **lo parcial acumulado** con un flag de
//! incompleto, en lugar de descartar todo el progreso ya pagado en cuota.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sdp_core::{Comment, Commenter};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

const API: &str = "https://www.googleapis.com/youtube/v3";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum YoutubeError {
    #[error("error de red hablando con la YouTube Data API: {0}")]
    Http(#[from] reqwest::Error),
    #[error("la YouTube Data API respondió error{}: {message}", reason_suffix(reason))]
    Api {
        status: u16,
        reason: Option<String>,
        message: String,
    },
    #[error("respuesta inesperada de la API: {0}")]
    Shape(String),
}

fn reason_suffix(reason: &Option<String>) -> String {
    match reason {
        Some(r) => format!(" ({r})"),
        None => String::new(),
    }
}

impl YoutubeError {
    /// ¿Es un agotamiento de cuota? (motivo para devolver parciales, no abortar).
    pub fn is_quota_exceeded(&self) -> bool {
        matches!(self, YoutubeError::Api { reason: Some(r), .. } if r == "quotaExceeded")
    }

    /// ¿Son los comentarios deshabilitados en ESTE video? (se saltea el video).
    pub fn is_comments_disabled(&self) -> bool {
        matches!(self, YoutubeError::Api { reason: Some(r), .. } if r == "commentsDisabled")
    }
}

/// Lo que devuelve una ingesta. `incomplete` marca que se cortó a mitad
/// (típicamente por cuota) y los datos son **parciales pero válidos** (F4).
#[derive(Debug, Default)]
pub struct Ingested {
    pub commenters: Vec<Commenter>,
    pub comments: Vec<Comment>,
    pub incomplete: bool,
    /// Detalle legible de por qué quedó incompleto (None si está completo).
    pub incomplete_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Tipos de la respuesta de la API (solo los campos que usamos).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorBody,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    code: u16,
    message: String,
    #[serde(default)]
    errors: Vec<ApiErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommentThreadList {
    #[serde(default)]
    items: Vec<CommentThread>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommentThread {
    snippet: ThreadSnippet,
}

#[derive(Debug, Deserialize)]
struct ThreadSnippet {
    #[serde(rename = "topLevelComment")]
    top_level_comment: TopLevelComment,
}

#[derive(Debug, Deserialize)]
struct TopLevelComment {
    id: String,
    snippet: CommentSnippet,
}

/// `authorChannelId` viene como objeto `{ value }`, no string (igual que en la
/// versión Node: ojo con esto al aplanar).
#[derive(Debug, Deserialize)]
struct AuthorChannelId {
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommentSnippet {
    #[serde(rename = "videoId")]
    video_id: Option<String>,
    #[serde(rename = "authorChannelId")]
    author_channel_id: Option<AuthorChannelId>,
    #[serde(rename = "authorDisplayName")]
    author_display_name: Option<String>,
    #[serde(rename = "authorProfileImageUrl")]
    author_profile_image_url: Option<String>,
    #[serde(rename = "authorChannelUrl")]
    author_channel_url: Option<String>,
    #[serde(rename = "textDisplay")]
    text_display: Option<String>,
    #[serde(rename = "textOriginal")]
    text_original: Option<String>,
    #[serde(rename = "likeCount", default)]
    like_count: u64,
    #[serde(rename = "publishedAt")]
    published_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct ChannelList {
    #[serde(default)]
    items: Vec<ChannelItem>,
}

#[derive(Debug, Deserialize)]
struct ChannelItem {
    #[serde(rename = "contentDetails")]
    content_details: ChannelContentDetails,
}

#[derive(Debug, Deserialize)]
struct ChannelContentDetails {
    #[serde(rename = "relatedPlaylists")]
    related_playlists: RelatedPlaylists,
}

#[derive(Debug, Deserialize)]
struct RelatedPlaylists {
    uploads: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlaylistItemList {
    #[serde(default)]
    items: Vec<PlaylistItem>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlaylistItem {
    #[serde(rename = "contentDetails")]
    content_details: PlaylistItemContentDetails,
}

#[derive(Debug, Deserialize)]
struct PlaylistItemContentDetails {
    #[serde(rename = "videoId")]
    video_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Mapeo PURO (sin red) — testeable con fixtures.
// ---------------------------------------------------------------------------

/// Aplana un `commentThread` a `(Comment, Commenter)`. Equivalente a
/// `flattenThread` + `mapper.js` del sidecar Node, en un solo paso tipado.
fn thread_to_models(thread: &CommentThread) -> (Comment, Commenter) {
    let top = &thread.snippet.top_level_comment;
    let s = &top.snippet;
    let author_id = s
        .author_channel_id
        .as_ref()
        .and_then(|a| a.value.clone())
        .unwrap_or_default();

    let comment = Comment {
        id: top.id.clone(),
        video_id: s.video_id.clone().unwrap_or_default(),
        author_channel_id: author_id.clone(),
        text: s
            .text_display
            .clone()
            .or_else(|| s.text_original.clone())
            .unwrap_or_default(),
        like_count: s.like_count,
        published_at: s.published_at.unwrap_or_else(Utc::now),
    };
    let commenter = Commenter {
        channel_id: author_id,
        display_name: s.author_display_name.clone().unwrap_or_default(),
        profile_image_url: s.author_profile_image_url.clone(),
        channel_url: s.author_channel_url.clone(),
    };
    (comment, commenter)
}

/// Junta comentarios de varios threads, deduplicando comentaristas por
/// `channel_id` (una persona comenta muchas veces). PURO.
fn collect(threads: &[CommentThread]) -> (Vec<Comment>, Vec<Commenter>) {
    let mut comments = Vec::with_capacity(threads.len());
    let mut commenters: Vec<Commenter> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in threads {
        let (comment, commenter) = thread_to_models(t);
        if seen.insert(commenter.channel_id.clone()) {
            commenters.push(commenter);
        }
        comments.push(comment);
    }
    (comments, commenters)
}

// ---------------------------------------------------------------------------
// Cliente HTTP.
// ---------------------------------------------------------------------------

/// Cliente de la YouTube Data API v3. La API key se guarda envuelta en
/// `SecretString` (F2): no se loguea ni se imprime por `Debug`, y se zeroiza al
/// dropearse. Se expone solo el tiempo justo para encodearla en la URL.
pub struct YoutubeClient {
    http: reqwest::Client,
    api_key: SecretString,
}

impl YoutubeClient {
    pub fn new(api_key: SecretString) -> Result<Self, YoutubeError> {
        let http = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?;
        Ok(Self { http, api_key })
    }

    /// GET + parseo tipado, clasificando errores de la API (HTTP no-2xx) a
    /// `YoutubeError::Api` con su `reason` (p.ej. `quotaExceeded`).
    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, YoutubeError> {
        // Construimos el query manualmente para incluir la key sin que quede
        // pegada como campo persistente. `encode` evita inyección por id raro.
        let mut url = format!("{API}/{path}?key={}", urlencoding::encode(self.key()));
        for (k, v) in params {
            if !v.is_empty() {
                url.push('&');
                url.push_str(&urlencoding::encode(k));
                url.push('=');
                url.push_str(&urlencoding::encode(v));
            }
        }

        let resp = self.http.get(&url).send().await?;
        let status = resp.status();
        let body = resp.bytes().await?;

        if !status.is_success() {
            return Err(parse_api_error(status.as_u16(), &body));
        }
        serde_json::from_slice::<T>(&body)
            .map_err(|e| YoutubeError::Shape(format!("no se pudo parsear {path}: {e}")))
    }

    /// Expone la key el menor tiempo posible (solo para encodearla en la URL).
    fn key(&self) -> &str {
        self.api_key.expose_secret()
    }

    /// Trae TODOS los comentarios top-level de un video (paginado).
    async fn comments_for_video(&self, video_id: &str) -> Result<Vec<CommentThread>, YoutubeError> {
        let mut out = Vec::new();
        let mut page_token = String::new();
        loop {
            let list: CommentThreadList = self
                .get(
                    "commentThreads",
                    &[
                        ("videoId", video_id),
                        ("part", "snippet"),
                        ("maxResults", "100"),
                        ("textFormat", "plainText"),
                        ("pageToken", &page_token),
                    ],
                )
                .await?;
            out.extend(list.items);
            match list.next_page_token {
                Some(t) => page_token = t,
                None => break,
            }
        }
        Ok(out)
    }

    /// IDs de video de un canal vía su playlist de uploads (paginado).
    async fn video_ids_for_channel(&self, channel_id: &str) -> Result<Vec<String>, YoutubeError> {
        let channels: ChannelList = self
            .get("channels", &[("id", channel_id), ("part", "contentDetails")])
            .await?;
        let uploads = channels
            .items
            .first()
            .and_then(|c| c.content_details.related_playlists.uploads.clone())
            .ok_or_else(|| {
                YoutubeError::Shape(format!("canal sin playlist de uploads: {channel_id}"))
            })?;

        let mut ids = Vec::new();
        let mut page_token = String::new();
        loop {
            let list: PlaylistItemList = self
                .get(
                    "playlistItems",
                    &[
                        ("playlistId", &uploads),
                        ("part", "contentDetails"),
                        ("maxResults", "50"),
                        ("pageToken", &page_token),
                    ],
                )
                .await?;
            for item in list.items {
                if let Some(v) = item.content_details.video_id {
                    ids.push(v);
                }
            }
            match list.next_page_token {
                Some(t) => page_token = t,
                None => break,
            }
        }
        Ok(ids)
    }

    /// Ingesta de un solo video. Sin acumulación parcial: si falla, falla entero
    /// (es una sola unidad de trabajo, no hay progreso que preservar).
    pub async fn ingest_video(&self, video_id: &str) -> Result<Ingested, YoutubeError> {
        let threads = self.comments_for_video(video_id).await?;
        let (comments, commenters) = collect(&threads);
        Ok(Ingested {
            commenters,
            comments,
            incomplete: false,
            incomplete_reason: None,
        })
    }

    /// Ingesta de TODOS los videos de un canal, **resiliente** (F4):
    /// - saltea videos con comentarios deshabilitados (`commentsDisabled`);
    /// - ante `quotaExceeded` (o cualquier otro error a mitad), devuelve lo
    ///   parcial acumulado con `incomplete = true` en vez de descartar todo;
    /// - la lista de IDs sí es prerrequisito: si falla traerla, no hay parcial
    ///   posible y se propaga el error.
    pub async fn ingest_channel(&self, channel_id: &str) -> Result<Ingested, YoutubeError> {
        let video_ids = self.video_ids_for_channel(channel_id).await?;

        let mut threads = Vec::new();
        let mut incomplete = false;
        let mut incomplete_reason = None;

        for video_id in &video_ids {
            match self.comments_for_video(video_id).await {
                Ok(mut v) => threads.append(&mut v),
                Err(e) if e.is_comments_disabled() => continue,
                Err(e) => {
                    // Cualquier otro error (típicamente cuota): cortamos pero
                    // conservamos lo ya traído. No perder trabajo pagado en cuota.
                    incomplete = true;
                    incomplete_reason = Some(e.to_string());
                    break;
                }
            }
        }

        let (comments, commenters) = collect(&threads);
        Ok(Ingested {
            commenters,
            comments,
            incomplete,
            incomplete_reason,
        })
    }
}

/// Clasifica el cuerpo de un error de la API a `YoutubeError::Api`, extrayendo
/// el `reason` del primer detalle (`quotaExceeded`, `commentsDisabled`, …).
/// PURO — testeable con fixtures de error sin red.
fn parse_api_error(status: u16, body: &[u8]) -> YoutubeError {
    match serde_json::from_slice::<ApiError>(body) {
        Ok(parsed) => {
            let reason = parsed
                .error
                .errors
                .into_iter()
                .find_map(|d| d.reason)
                .filter(|r| !r.is_empty());
            YoutubeError::Api {
                status: parsed.error.code,
                reason,
                message: parsed.error.message,
            }
        }
        Err(_) => YoutubeError::Api {
            status,
            reason: None,
            message: format!("HTTP {status}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixture: un commentThread tal como lo devuelve commentThreads (part=snippet).
    const THREAD_JSON: &str = r#"{
      "snippet": {
        "topLevelComment": {
          "id": "Ugx-comment-1",
          "snippet": {
            "videoId": "vid1",
            "textDisplay": "hola",
            "textOriginal": "hola",
            "authorDisplayName": "Ana",
            "authorProfileImageUrl": "http://img/ana.png",
            "authorChannelUrl": "http://yt/ana",
            "authorChannelId": { "value": "UCana" },
            "likeCount": 5,
            "publishedAt": "2021-09-27T03:00:00Z"
          }
        }
      }
    }"#;

    fn thread() -> CommentThread {
        serde_json::from_str(THREAD_JSON).unwrap()
    }

    #[test]
    fn aplana_thread_a_modelos_del_core() {
        let (comment, commenter) = thread_to_models(&thread());
        assert_eq!(comment.id, "Ugx-comment-1");
        assert_eq!(comment.video_id, "vid1");
        // Clave: authorChannelId.value, no "[object Object]".
        assert_eq!(comment.author_channel_id, "UCana");
        assert_eq!(comment.text, "hola");
        assert_eq!(comment.like_count, 5);
        assert_eq!(commenter.channel_id, "UCana");
        assert_eq!(commenter.display_name, "Ana");
        assert_eq!(commenter.profile_image_url.as_deref(), Some("http://img/ana.png"));
    }

    #[test]
    fn parsea_lista_de_threads_paginada() {
        let list: CommentThreadList = serde_json::from_str(&format!(
            r#"{{ "items": [{THREAD_JSON}], "nextPageToken": "ABC" }}"#
        ))
        .unwrap();
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.next_page_token.as_deref(), Some("ABC"));
    }

    #[test]
    fn collect_dedup_comentaristas_por_channel_id() {
        // misma persona en dos threads: 2 comentarios, 1 comentarista.
        let (comments, commenters) = collect(&[thread(), thread()]);
        assert_eq!(comments.len(), 2);
        assert_eq!(commenters.len(), 1);
        assert_eq!(commenters[0].channel_id, "UCana");
    }

    #[test]
    fn clasifica_quota_exceeded() {
        let body = br#"{ "error": { "code": 403, "message": "quota",
            "errors": [{ "reason": "quotaExceeded" }] } }"#;
        let err = parse_api_error(403, body);
        assert!(err.is_quota_exceeded());
        assert!(!err.is_comments_disabled());
        match err {
            YoutubeError::Api { status, reason, .. } => {
                assert_eq!(status, 403);
                assert_eq!(reason.as_deref(), Some("quotaExceeded"));
            }
            other => panic!("esperaba Api, fue {other:?}"),
        }
    }

    #[test]
    fn clasifica_comments_disabled() {
        let body = br#"{ "error": { "code": 403, "message": "off",
            "errors": [{ "reason": "commentsDisabled" }] } }"#;
        let err = parse_api_error(403, body);
        assert!(err.is_comments_disabled());
        assert!(!err.is_quota_exceeded());
    }

    #[test]
    fn error_no_json_cae_a_http_status() {
        let err = parse_api_error(500, b"<html>oops</html>");
        assert!(!err.is_quota_exceeded());
        match err {
            YoutubeError::Api { status, reason, .. } => {
                assert_eq!(status, 500);
                assert_eq!(reason, None);
            }
            other => panic!("esperaba Api, fue {other:?}"),
        }
    }

    #[test]
    fn la_key_no_se_filtra_por_debug() {
        // SecretString no debe imprimir el valor en claro (F2).
        let key = SecretString::from("AIza-super-secreta".to_string());
        let dbg = format!("{key:?}");
        assert!(!dbg.contains("AIza-super-secreta"), "la key se filtró: {dbg}");
    }
}
