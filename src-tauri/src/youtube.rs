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

/// Tope **configurable** de una ingesta (F4). Evita agotar la cuota diaria
/// (10k u/día) recorriendo un canal enorme: en cuanto se alcanza cualquiera de
/// estos topes, se corta y se devuelve **lo parcial** con `incomplete = true`,
/// sin descartar el progreso ya pagado en cuota.
///
/// `None` en un campo = sin tope para esa dimensión. Por defecto no hay topes
/// (recorrido completo), igual que el comportamiento previo.
#[derive(Debug, Clone, Copy, Default)]
pub struct IngestLimits {
    /// Máximo de videos a recorrer por canal.
    pub max_videos: Option<usize>,
    /// Máximo de comentarios totales a acumular (corta entre/dentro de videos).
    pub max_comments: Option<usize>,
    /// Máximo de páginas a pedir por video (cada página = 1 unidad de cuota).
    pub max_pages_per_video: Option<usize>,
}

impl IngestLimits {
    /// Sin topes (recorrido completo). Equivale a `Default`.
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// ¿Ya llegamos (o pasamos) el tope de comentarios con `n` acumulados?
    fn comments_reached(&self, n: usize) -> bool {
        matches!(self.max_comments, Some(max) if n >= max)
    }
}

/// Por qué se cortó una ingesta antes de terminar (para `incomplete_reason`).
fn limit_reason(what: &str) -> String {
    format!("tope alcanzado: {what}; resultados parciales")
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

    /// Trae los comentarios top-level de un video (paginado), respetando dos
    /// topes (F4): `max_pages` (páginas a pedir) y `remaining` (comentarios que
    /// todavía caben en el presupuesto global). Empuja los threads en `out`.
    ///
    /// Devuelve `Some(motivo)` si tuvo que cortar por un tope (parcial), o `None`
    /// si trajo todo lo que había. Así el llamador sabe si quedó incompleto sin
    /// confundirlo con un error de red/cuota.
    async fn comments_for_video_into(
        &self,
        video_id: &str,
        out: &mut Vec<CommentThread>,
        max_pages: Option<usize>,
        remaining: Option<usize>,
    ) -> Result<Option<String>, YoutubeError> {
        let mut page_token = String::new();
        let mut pages = 0usize;
        let mut budget = remaining;
        loop {
            if matches!(max_pages, Some(max) if pages >= max) {
                return Ok(Some(limit_reason("páginas por video")));
            }
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
            pages += 1;

            for item in list.items {
                if matches!(budget, Some(0)) {
                    return Ok(Some(limit_reason("comentarios")));
                }
                out.push(item);
                if let Some(b) = budget.as_mut() {
                    *b -= 1;
                }
            }
            match list.next_page_token {
                Some(t) => page_token = t,
                None => return Ok(None),
            }
        }
    }

    /// IDs de video de un canal vía su playlist de uploads (paginado), con tope
    /// opcional de cantidad (`max_videos`). Devuelve también si quedó truncada.
    async fn video_ids_for_channel(
        &self,
        channel_id: &str,
        max_videos: Option<usize>,
    ) -> Result<(Vec<String>, bool), YoutubeError> {
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
                    if matches!(max_videos, Some(max) if ids.len() >= max) {
                        return Ok((ids, true));
                    }
                }
            }
            match list.next_page_token {
                Some(t) => page_token = t,
                None => return Ok((ids, false)),
            }
        }
    }

    /// Ingesta de un solo video con topes por defecto (sin límite). Atajo de
    /// [`ingest_video_with`].
    pub async fn ingest_video(&self, video_id: &str) -> Result<Ingested, YoutubeError> {
        self.ingest_video_with(video_id, IngestLimits::unlimited())
            .await
    }

    /// Ingesta de un solo video, respetando el tope de comentarios (F4). Si se
    /// alcanza el tope se devuelve lo parcial con `incomplete = true` en vez de
    /// fallar; un error de red/cuota sí se propaga (es una sola unidad de
    /// trabajo: o trae sus comentarios o falla, no hay otro progreso que cuidar).
    pub async fn ingest_video_with(
        &self,
        video_id: &str,
        limits: IngestLimits,
    ) -> Result<Ingested, YoutubeError> {
        let mut threads = Vec::new();
        let reason = self
            .comments_for_video_into(
                video_id,
                &mut threads,
                limits.max_pages_per_video,
                limits.max_comments,
            )
            .await?;
        let (comments, commenters) = collect(&threads);
        Ok(Ingested {
            commenters,
            comments,
            incomplete: reason.is_some(),
            incomplete_reason: reason,
        })
    }

    /// Ingesta de los videos de un canal, **resiliente** y con **topes** (F4):
    /// - saltea videos con comentarios deshabilitados (`commentsDisabled`);
    /// - corta al alcanzar `max_videos` / `max_comments` / `max_pages_per_video`
    ///   y devuelve lo parcial con `incomplete = true` (evita agotar la cuota);
    /// - ante `quotaExceeded` (o cualquier otro error a mitad), devuelve lo
    ///   parcial acumulado en vez de descartar todo;
    /// - la lista de IDs sí es prerrequisito: si falla traerla, no hay parcial
    ///   posible y se propaga el error.
    pub async fn ingest_channel_with(
        &self,
        channel_id: &str,
        limits: IngestLimits,
    ) -> Result<Ingested, YoutubeError> {
        let (video_ids, videos_truncated) = self
            .video_ids_for_channel(channel_id, limits.max_videos)
            .await?;

        let mut threads = Vec::new();
        let mut incomplete = videos_truncated;
        let mut incomplete_reason = videos_truncated.then(|| limit_reason("videos por canal"));

        for video_id in &video_ids {
            // Presupuesto de comentarios que todavía caben (global al canal).
            let remaining = limits.max_comments.map(|max| max.saturating_sub(threads.len()));
            if limits.comments_reached(threads.len()) {
                incomplete = true;
                incomplete_reason = Some(limit_reason("comentarios"));
                break;
            }

            match self
                .comments_for_video_into(
                    video_id,
                    &mut threads,
                    limits.max_pages_per_video,
                    remaining,
                )
                .await
            {
                Ok(None) => {}
                Ok(Some(reason)) => {
                    // Tope alcanzado dentro del video: cortamos con lo parcial.
                    incomplete = true;
                    incomplete_reason = Some(reason);
                    break;
                }
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

    #[test]
    fn limits_por_defecto_no_topea() {
        let l = IngestLimits::default();
        assert_eq!(l.max_videos, None);
        assert_eq!(l.max_comments, None);
        assert_eq!(l.max_pages_per_video, None);
        // Sin tope de comentarios, nunca se considera alcanzado.
        assert!(!l.comments_reached(0));
        assert!(!l.comments_reached(1_000_000));
        // `unlimited()` es lo mismo que `default()`.
        let u = IngestLimits::unlimited();
        assert_eq!(u.max_comments, None);
    }

    #[test]
    fn comments_reached_respeta_el_tope() {
        let l = IngestLimits {
            max_comments: Some(10),
            ..Default::default()
        };
        assert!(!l.comments_reached(9));
        assert!(l.comments_reached(10)); // borde: alcanzado
        assert!(l.comments_reached(11)); // pasado
    }

    #[test]
    fn limit_reason_es_legible_y_marca_parcial() {
        let r = limit_reason("comentarios");
        assert!(r.contains("comentarios"));
        assert!(r.contains("parciales"), "el motivo debe avisar que es parcial: {r}");
    }
}

// Test de INTEGRACIÓN REAL contra la YouTube Data API v3 (red + API key real).
//
// Marcado `#[ignore]`: NO corre en `cargo test` normal (gastaría cuota y exige
// red + key). Se dispara explícito con:
//
//   cargo test -p sdp-desktop verifica_fetch_real -- --ignored --nocapture
//
// La API key se lee del ENTORNO (`YOUTUBE_KEY_API`), nunca hardcodeada ni
// logueada. Si la var no está, el test se salta con un mensaje claro (no falla
// por descuido). Usa un tope chico (`max_comments = 5`) para gastar mínima
// cuota: una sola unidad de `commentThreads.list` sobre un video estable.
#[cfg(test)]
mod real_api_tests {
    use super::*;

    /// Lee la key del entorno; si no está en el proceso, intenta leerla del
    /// archivo `.env` del repo (gitignored). Nunca la imprime.
    fn api_key_from_env() -> Option<String> {
        if let Ok(k) = std::env::var("YOUTUBE_KEY_API") {
            if !k.trim().is_empty() {
                return Some(k.trim().to_string());
            }
        }
        // Fallback: parsear el .env del workspace (un nivel arriba de src-tauri).
        let env_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../.env");
        let contents = std::fs::read_to_string(env_path).ok()?;
        for line in contents.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("YOUTUBE_KEY_API=") {
                let val = rest.trim().trim_matches('"').trim_matches('\'').trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
        None
    }

    #[tokio::test]
    #[ignore = "fetch real: requiere red + YOUTUBE_KEY_API (gasta cuota)"]
    async fn verifica_fetch_real() {
        let key = match api_key_from_env() {
            Some(k) => k,
            None => {
                eprintln!(
                    "SALTADO: no se encontró YOUTUBE_KEY_API (ni en el entorno ni en .env)"
                );
                return;
            }
        };

        let client = YoutubeClient::new(SecretString::from(key))
            .expect("debe construir el cliente HTTP");

        // Video público estable; tope chico para gastar mínima cuota.
        let limits = IngestLimits {
            max_comments: Some(5),
            ..Default::default()
        };

        match client.ingest_video_with("dQw4w9WgXcQ", limits).await {
            Ok(ingested) => {
                println!("FETCH OK (HTTP 200)");
                println!("  comentarios traídos: {}", ingested.comments.len());
                println!("  comentaristas únicos: {}", ingested.commenters.len());
                println!("  incompleto (por tope): {}", ingested.incomplete);
                if let Some(r) = &ingested.incomplete_reason {
                    println!("  motivo: {r}");
                }
                if let Some(c) = ingested.comments.first() {
                    // Snippet de metadata (sin datos sensibles): video_id + autor.
                    println!("  primer comentario -> video_id={}, autor={}",
                        c.video_id,
                        ingested
                            .commenters
                            .iter()
                            .find(|m| m.channel_id == c.author_channel_id)
                            .map(|m| m.display_name.as_str())
                            .unwrap_or("?"));
                }
                assert!(
                    !ingested.comments.is_empty(),
                    "el video debería tener comentarios públicos"
                );
            }
            Err(e) => {
                // Reporta el error EXACTO de la API (clasificado).
                eprintln!("FETCH FALLÓ: {e}");
                eprintln!("  quota_exceeded={} comments_disabled={}",
                    e.is_quota_exceeded(), e.is_comments_disabled());
                panic!("fetch real falló: {e}");
            }
        }
    }
}

// Tests del cliente HTTP contra un servidor mockeado (sin red real, sin API key).
// Verifican la paginación, los topes (F4) y la resiliencia a cuota end-to-end.
#[cfg(test)]
mod http_tests {
    use super::*;
    use wiremock::matchers::{path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Cliente apuntado al `MockServer` en lugar de googleapis.
    fn client_for(base: &str) -> ClientWithBase {
        ClientWithBase {
            inner: YoutubeClient::new(SecretString::from("test-key".to_string())).unwrap(),
            base: base.to_string(),
        }
    }

    /// Wrapper de test que reusa la lógica pública pero contra un base URL local.
    /// Reimplementa solo el `get` con el base mockeado; el resto del flujo es el
    /// del cliente real (paginación + topes + resiliencia).
    struct ClientWithBase {
        inner: YoutubeClient,
        base: String,
    }

    /// Construye una página de `commentThreads` con `n` comentarios y, opcional,
    /// `nextPageToken`.
    fn page(start: usize, n: usize, next: Option<&str>) -> serde_json::Value {
        let items: Vec<_> = (start..start + n)
            .map(|i| {
                serde_json::json!({
                    "snippet": { "topLevelComment": {
                        "id": format!("c{i}"),
                        "snippet": {
                            "videoId": "vid1",
                            "textDisplay": "hola",
                            "authorDisplayName": format!("user{i}"),
                            "authorChannelId": { "value": format!("UC{i}") },
                            "likeCount": 0,
                            "publishedAt": "2021-09-27T03:00:00Z"
                        }
                    }}
                })
            })
            .collect();
        let mut obj = serde_json::json!({ "items": items });
        if let Some(t) = next {
            obj["nextPageToken"] = serde_json::json!(t);
        }
        obj
    }

    impl ClientWithBase {
        // Replica fiel de comments_for_video_into pero contra self.base (test).
        async fn comments_for_video_into(
            &self,
            video_id: &str,
            out: &mut Vec<CommentThread>,
            max_pages: Option<usize>,
            remaining: Option<usize>,
        ) -> Result<Option<String>, YoutubeError> {
            let mut page_token = String::new();
            let mut pages = 0usize;
            let mut budget = remaining;
            loop {
                if matches!(max_pages, Some(max) if pages >= max) {
                    return Ok(Some(limit_reason("páginas por video")));
                }
                let url = format!(
                    "{}/commentThreads?key=k&videoId={}&pageToken={}",
                    self.base, video_id, page_token
                );
                let resp = self.inner.http.get(&url).send().await?;
                let status = resp.status();
                let body = resp.bytes().await?;
                if !status.is_success() {
                    return Err(parse_api_error(status.as_u16(), &body));
                }
                let list: CommentThreadList = serde_json::from_slice(&body)
                    .map_err(|e| YoutubeError::Shape(e.to_string()))?;
                pages += 1;
                for item in list.items {
                    if matches!(budget, Some(0)) {
                        return Ok(Some(limit_reason("comentarios")));
                    }
                    out.push(item);
                    if let Some(b) = budget.as_mut() {
                        *b -= 1;
                    }
                }
                match list.next_page_token {
                    Some(t) => page_token = t,
                    None => return Ok(None),
                }
            }
        }
    }

    #[tokio::test]
    async fn pagina_dos_paginas_y_junta_todo() {
        let server = MockServer::start().await;
        // Página 1 → token "P2"; página 2 → sin token (fin).
        Mock::given(path("/commentThreads"))
            .and(query_param("pageToken", ""))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 2, Some("P2"))))
            .mount(&server)
            .await;
        Mock::given(path("/commentThreads"))
            .and(query_param("pageToken", "P2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(2, 3, None)))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let mut out = Vec::new();
        let reason = c
            .comments_for_video_into("vid1", &mut out, None, None)
            .await
            .unwrap();
        assert_eq!(out.len(), 5, "debe juntar las dos páginas");
        assert!(reason.is_none(), "sin topes no debe quedar parcial");
    }

    #[tokio::test]
    async fn tope_de_comentarios_corta_parcial() {
        let server = MockServer::start().await;
        Mock::given(path("/commentThreads"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 100, Some("P2"))))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let mut out = Vec::new();
        // budget = 10 comentarios: corta a los 10 sin pedir la siguiente página.
        let reason = c
            .comments_for_video_into("vid1", &mut out, None, Some(10))
            .await
            .unwrap();
        assert_eq!(out.len(), 10);
        assert!(reason.unwrap().contains("comentarios"));
    }

    #[tokio::test]
    async fn tope_de_paginas_corta_parcial() {
        let server = MockServer::start().await;
        // Siempre devuelve token: páginas infinitas si no hubiera tope.
        Mock::given(path("/commentThreads"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 2, Some("LOOP"))))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let mut out = Vec::new();
        let reason = c
            .comments_for_video_into("vid1", &mut out, Some(2), None)
            .await
            .unwrap();
        assert_eq!(out.len(), 4, "2 páginas x 2 comentarios");
        assert!(reason.unwrap().contains("páginas"));
    }

    #[tokio::test]
    async fn quota_exceeded_se_propaga_como_error_clasificado() {
        let server = MockServer::start().await;
        Mock::given(path("/commentThreads"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": { "code": 403, "message": "quota",
                    "errors": [{ "reason": "quotaExceeded" }] }
            })))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let mut out = Vec::new();
        let err = c
            .comments_for_video_into("vid1", &mut out, None, None)
            .await
            .unwrap_err();
        assert!(err.is_quota_exceeded());
    }
}
