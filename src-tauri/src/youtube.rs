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
use sdp_core::{Comment, Commenter, SearchHit, SearchPlan, VideoMeta};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

const API: &str = "https://www.googleapis.com/youtube/v3";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum YoutubeError {
    #[error("error de red hablando con la YouTube Data API: {0}")]
    Http(#[from] reqwest::Error),
    #[error(
        "la YouTube Data API respondió error{}: {message}",
        reason_suffix(reason)
    )]
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

// --- videos.list (F9): metadata de videos (1 unidad por request, hasta 50 ids).
// Consumido por `fetch_video_meta`, cableado en `lib.rs` detrás del gate de costo.

#[derive(Debug, Deserialize)]
struct VideoListResponse {
    #[serde(default)]
    items: Vec<VideoItem>,
}

#[derive(Debug, Deserialize)]
struct VideoItem {
    id: String,
    snippet: VideoSnippet,
    /// Ausente si el video oculta sus estadísticas.
    #[serde(default)]
    statistics: Option<VideoStatistics>,
}

#[derive(Debug, Deserialize)]
struct VideoSnippet {
    #[serde(rename = "channelId")]
    channel_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(rename = "publishedAt")]
    published_at: Option<DateTime<Utc>>,
}

/// Las cuentas vienen como **string** en la API (no número): se parsean en el
/// mapeo puro. Si el video las oculta, el objeto entero falta.
#[derive(Debug, Deserialize)]
struct VideoStatistics {
    #[serde(rename = "viewCount", default)]
    view_count: Option<String>,
    #[serde(rename = "likeCount", default)]
    like_count: Option<String>,
    #[serde(rename = "commentCount", default)]
    comment_count: Option<String>,
}

// --- search.list (F10): descubrimiento/trending (100 unidades por página).
// Consumido por `search`, cableado en `lib.rs` detrás del gate de costo (run_search).

#[derive(Debug, Deserialize)]
struct SearchListResponse {
    #[serde(default)]
    items: Vec<SearchItem>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchItem {
    id: SearchId,
    snippet: SearchSnippet,
}

/// `search.list` puede devolver canales o playlists además de videos; sólo nos
/// quedamos con los que traen `videoId` (ver `search_item_to_hit`).
#[derive(Debug, Deserialize)]
struct SearchId {
    #[serde(rename = "videoId")]
    video_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchSnippet {
    #[serde(rename = "channelId")]
    channel_id: Option<String>,
    title: Option<String>,
    #[serde(rename = "publishedAt")]
    published_at: Option<DateTime<Utc>>,
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

/// Mapea un `videos.list` item al modelo de dominio `VideoMeta` (F9). PURO:
/// testeable con un fixture JSON sin red. Las cuentas vienen como string en la
/// API y acá se parsean a `u64`; si el video oculta estadísticas quedan en
/// `None` (no se inventan ceros).
fn video_item_to_meta(item: &VideoItem) -> VideoMeta {
    let s = &item.snippet;
    let stats = item.statistics.as_ref();
    let count = |field: &Option<String>| field.as_ref().and_then(|n| n.parse::<u64>().ok());
    VideoMeta {
        video_id: item.id.clone(),
        channel_id: s.channel_id.clone().unwrap_or_default(),
        title: s.title.clone().unwrap_or_default(),
        description: s.description.clone().unwrap_or_default(),
        tags: s.tags.clone(),
        view_count: stats.and_then(|st| count(&st.view_count)),
        like_count: stats.and_then(|st| count(&st.like_count)),
        comment_count: stats.and_then(|st| count(&st.comment_count)),
        published_at: s.published_at.unwrap_or_else(Utc::now),
    }
}

/// Mapea un item de `search.list` a `SearchHit` (F10). Devuelve `None` para los
/// resultados que no son videos (canales/playlists: sin `videoId`), que el
/// llamador descarta. PURO.
fn search_item_to_hit(item: &SearchItem) -> Option<SearchHit> {
    let video_id = item.id.video_id.clone()?;
    let s = &item.snippet;
    Some(SearchHit {
        video_id,
        channel_id: s.channel_id.clone().unwrap_or_default(),
        title: s.title.clone().unwrap_or_default(),
        published_at: s.published_at.unwrap_or_else(Utc::now),
    })
}

/// Resultado de una búsqueda (F10). Igual que [`Ingested`], `incomplete` marca
/// que se cortó por cuota (`quotaExceeded`) devolviendo lo parcial ya pagado,
/// en vez de descartar el progreso.
#[derive(Debug, Default, serde::Serialize)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub incomplete: bool,
    pub incomplete_reason: Option<String>,
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
    /// Base URL de la API. Configurable (`with_base`) para apuntar los tests a un
    /// `MockServer` sin tocar red real; en producción es [`API`].
    base: String,
}

impl YoutubeClient {
    pub fn new(api_key: SecretString) -> Result<Self, YoutubeError> {
        Self::with_base(api_key, API.to_string())
    }

    /// Igual que [`new`] pero con la base URL configurable. Permite que los tests
    /// con wiremock ejerciten los **métodos de producción** (no una reimplementación
    /// paralela), incluyendo el armado real de la URL y la clasificación de errores
    /// (auditoría P3).
    pub fn with_base(api_key: SecretString, base: String) -> Result<Self, YoutubeError> {
        let http = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?;
        Ok(Self {
            http,
            api_key,
            base,
        })
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
        let mut url = format!(
            "{}/{path}?key={}",
            self.base,
            urlencoding::encode(self.key())
        );
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
            // Corte por presupuesto agotado AL TOPE del loop, antes de pedir otra
            // página (auditoría P15): si la página anterior dejó budget==0 y hay
            // nextPageToken, NO gastamos una unidad de cuota extra para una página
            // que descartaríamos entera en su primer item.
            if matches!(budget, Some(0)) {
                return Ok(Some(limit_reason("comentarios")));
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
            .get(
                "channels",
                &[("id", channel_id), ("part", "contentDetails")],
            )
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
            let remaining = limits
                .max_comments
                .map(|max| max.saturating_sub(threads.len()));
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

    /// Trae metadata de videos por id (F9, `videos.list`). La API acepta hasta
    /// **50 ids por request** (cada request = 1 unidad de cuota), así que los
    /// `ids` se parten en chunks de 50 y se concatenan los resultados.
    ///
    /// Es una operación barata pero de red: el gate de costo (estimar →
    /// confirmar) y la persistencia viven en capas de arriba (`lib.rs` /
    /// `sdp-storage`); acá solo el efecto de red + mapeo puro (boundary).
    ///
    /// No falla por ids inexistentes: la API simplemente los omite de `items`,
    /// así que el resultado puede tener menos elementos que `ids`.
    pub async fn fetch_video_meta(&self, ids: &[String]) -> Result<Vec<VideoMeta>, YoutubeError> {
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(50) {
            let joined = chunk.join(",");
            let list: VideoListResponse = self
                .get(
                    "videos",
                    &[("id", joined.as_str()), ("part", "snippet,statistics")],
                )
                .await?;
            out.extend(list.items.iter().map(video_item_to_meta));
        }
        Ok(out)
    }

    /// Búsqueda/trending (F10, `search.list`). **Cara**: 100 unidades por página,
    /// por eso `plan.max_pages` es el tope de costo (el gate lo confirma antes).
    ///
    /// Reusa la resiliencia de cuota (F4): ante `quotaExceeded` a mitad de la
    /// paginación devuelve lo parcial con `incomplete = true` en vez de fallar.
    /// Cualquier otro error de red sí se propaga. `max_pages = 0` no toca la red.
    pub async fn search(&self, plan: &SearchPlan) -> Result<SearchResults, YoutubeError> {
        // `trending` ordena por lo más visto; si no, por relevancia textual.
        let order = if plan.trending {
            "viewCount"
        } else {
            "relevance"
        };
        let mut hits = Vec::new();
        let mut page_token = String::new();
        let mut pages = 0u32;

        while pages < plan.max_pages {
            let list: SearchListResponse = match self
                .get(
                    "search",
                    &[
                        ("q", plan.query.as_str()),
                        ("part", "snippet"),
                        ("type", "video"),
                        ("order", order),
                        ("maxResults", "50"),
                        ("pageToken", &page_token),
                    ],
                )
                .await
            {
                Ok(list) => list,
                Err(e) if e.is_quota_exceeded() => {
                    // Cuota agotada a mitad: conservamos lo ya traído.
                    return Ok(SearchResults {
                        hits,
                        incomplete: true,
                        incomplete_reason: Some(e.to_string()),
                    });
                }
                Err(e) => return Err(e),
            };
            pages += 1;
            hits.extend(list.items.iter().filter_map(search_item_to_hit));
            match list.next_page_token {
                Some(t) if pages < plan.max_pages => page_token = t,
                _ => break,
            }
        }

        Ok(SearchResults {
            hits,
            incomplete: false,
            incomplete_reason: None,
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
        assert_eq!(
            commenter.profile_image_url.as_deref(),
            Some("http://img/ana.png")
        );
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
        assert!(
            !dbg.contains("AIza-super-secreta"),
            "la key se filtró: {dbg}"
        );
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
        assert!(
            r.contains("parciales"),
            "el motivo debe avisar que es parcial: {r}"
        );
    }

    // Fixture: un item de videos.list (part=snippet,statistics) tal como lo
    // devuelve la API; ojo: las cuentas vienen como STRING.
    const VIDEO_ITEM_JSON: &str = r#"{
      "id": "vid1",
      "snippet": {
        "channelId": "UCcanal",
        "title": "Cómo usar señales en Angular",
        "description": "Tutorial completo",
        "tags": ["angular", "signals"],
        "publishedAt": "2021-09-27T03:00:00Z"
      },
      "statistics": {
        "viewCount": "1234",
        "likeCount": "56",
        "commentCount": "7"
      }
    }"#;

    #[test]
    fn mapea_video_item_a_meta_con_cuentas_string() {
        let item: VideoItem = serde_json::from_str(VIDEO_ITEM_JSON).unwrap();
        let meta = video_item_to_meta(&item);
        assert_eq!(meta.video_id, "vid1");
        assert_eq!(meta.channel_id, "UCcanal");
        assert_eq!(meta.title, "Cómo usar señales en Angular");
        assert_eq!(meta.tags, vec!["angular", "signals"]);
        // Clave: "1234" string -> 1234 u64.
        assert_eq!(meta.view_count, Some(1234));
        assert_eq!(meta.like_count, Some(56));
        assert_eq!(meta.comment_count, Some(7));
    }

    #[test]
    fn video_sin_statistics_mapea_a_none() {
        // Video con estadísticas ocultas: sin objeto `statistics`.
        let json = r#"{
          "id": "vid2",
          "snippet": {
            "channelId": "UCcanal",
            "title": "Privado",
            "publishedAt": "2021-09-27T03:00:00Z"
          }
        }"#;
        let item: VideoItem = serde_json::from_str(json).unwrap();
        let meta = video_item_to_meta(&item);
        assert_eq!(meta.view_count, None);
        assert_eq!(meta.like_count, None);
        assert_eq!(meta.comment_count, None);
        assert!(meta.tags.is_empty());
        assert!(meta.description.is_empty());
    }

    #[test]
    fn parsea_lista_de_videos() {
        let list: VideoListResponse =
            serde_json::from_str(&format!(r#"{{ "items": [{VIDEO_ITEM_JSON}] }}"#)).unwrap();
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].id, "vid1");
    }

    #[test]
    fn mapea_search_item_video_a_hit() {
        let item: SearchItem = serde_json::from_str(
            r#"{
              "id": { "videoId": "v1" },
              "snippet": {
                "channelId": "UCotro",
                "title": "Competencia",
                "publishedAt": "2021-09-27T03:00:00Z"
              }
            }"#,
        )
        .unwrap();
        let hit = search_item_to_hit(&item).expect("un video debe mapear a hit");
        assert_eq!(hit.video_id, "v1");
        assert_eq!(hit.channel_id, "UCotro");
        assert_eq!(hit.title, "Competencia");
    }

    #[test]
    fn search_item_no_video_se_descarta() {
        // Resultado de canal: id sin videoId -> None (se filtra).
        let item: SearchItem = serde_json::from_str(
            r#"{
              "id": { "channelId": "UCx" },
              "snippet": { "channelId": "UCx", "title": "Un canal",
                "publishedAt": "2021-09-27T03:00:00Z" }
            }"#,
        )
        .unwrap();
        assert!(search_item_to_hit(&item).is_none());
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

    /// Lee la key SOLO del entorno del proceso (`YOUTUBE_KEY_API`). Nunca la
    /// imprime ni la parsea de un `.env` en disco: un secreto en disco no debe
    /// cruzar al runtime de tests (auditoría P16). Para correr el test real,
    /// exportá la var antes (`YOUTUBE_KEY_API=… cargo test … -- --ignored`).
    fn api_key_from_env() -> Option<String> {
        std::env::var("YOUTUBE_KEY_API")
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
    }

    #[tokio::test]
    #[ignore = "fetch real: requiere red + YOUTUBE_KEY_API (gasta cuota)"]
    async fn verifica_fetch_real() {
        let key = match api_key_from_env() {
            Some(k) => k,
            None => {
                eprintln!("SALTADO: no se encontró YOUTUBE_KEY_API (ni en el entorno ni en .env)");
                return;
            }
        };

        let client =
            YoutubeClient::new(SecretString::from(key)).expect("debe construir el cliente HTTP");

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
                    println!(
                        "  primer comentario -> video_id={}, autor={}",
                        c.video_id,
                        ingested
                            .commenters
                            .iter()
                            .find(|m| m.channel_id == c.author_channel_id)
                            .map(|m| m.display_name.as_str())
                            .unwrap_or("?")
                    );
                }
                assert!(
                    !ingested.comments.is_empty(),
                    "el video debería tener comentarios públicos"
                );
            }
            Err(e) => {
                // Reporta el error EXACTO de la API (clasificado).
                eprintln!("FETCH FALLÓ: {e}");
                eprintln!(
                    "  quota_exceeded={} comments_disabled={}",
                    e.is_quota_exceeded(),
                    e.is_comments_disabled()
                );
                panic!("fetch real falló: {e}");
            }
        }
    }
}

// Tests del cliente HTTP contra un servidor mockeado (sin red real, sin API key).
// Ejercitan los **métodos de producción** de `YoutubeClient` vía `with_base`
// (auditoría P3/P4): paginación, topes (F4) y resiliencia de cuota end-to-end,
// incluyendo el armado real de la URL y la clasificación de errores.
#[cfg(test)]
mod http_tests {
    use super::*;
    use wiremock::matchers::{path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Cliente real apuntado al `MockServer` en lugar de googleapis.
    fn client_for(base: &str) -> YoutubeClient {
        YoutubeClient::with_base(SecretString::from("test-key".to_string()), base.to_string())
            .unwrap()
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

    /// Respuesta de `channels.list` con la playlist de uploads de un canal.
    fn channel_uploads(playlist: &str) -> serde_json::Value {
        serde_json::json!({
            "items": [{
                "contentDetails": { "relatedPlaylists": { "uploads": playlist } }
            }]
        })
    }

    /// Página de `playlistItems` con los `video_ids` dados y, opcional, token.
    fn playlist_page(video_ids: &[&str], next: Option<&str>) -> serde_json::Value {
        let items: Vec<_> = video_ids
            .iter()
            .map(|v| serde_json::json!({ "contentDetails": { "videoId": v } }))
            .collect();
        let mut obj = serde_json::json!({ "items": items });
        if let Some(t) = next {
            obj["nextPageToken"] = serde_json::json!(t);
        }
        obj
    }

    /// Body de error de la API con un `reason` (quotaExceeded/commentsDisabled).
    fn api_error_body(reason: &str) -> serde_json::Value {
        serde_json::json!({
            "error": { "code": 403, "message": "x", "errors": [{ "reason": reason }] }
        })
    }

    /// Una página de `search.list` con `n` videos y, opcional, `nextPageToken`.
    fn search_page(start: usize, n: usize, next: Option<&str>) -> serde_json::Value {
        let items: Vec<_> = (start..start + n)
            .map(|i| {
                serde_json::json!({
                    "id": { "videoId": format!("v{i}") },
                    "snippet": {
                        "channelId": "UCotro",
                        "title": format!("hit{i}"),
                        "publishedAt": "2021-09-27T03:00:00Z"
                    }
                })
            })
            .collect();
        let mut obj = serde_json::json!({ "items": items });
        if let Some(t) = next {
            obj["nextPageToken"] = serde_json::json!(t);
        }
        obj
    }

    #[tokio::test]
    async fn search_pagina_hasta_max_pages() {
        let server = MockServer::start().await;
        // 1ra página: SIN pageToken (el cliente real omite params vacíos en la URL).
        Mock::given(path("/search"))
            .and(query_param_is_missing("pageToken"))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_page(0, 2, Some("P2"))))
            .mount(&server)
            .await;
        Mock::given(path("/search"))
            .and(query_param("pageToken", "P2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_page(2, 2, None)))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let plan = SearchPlan {
            query: "x".into(),
            trending: false,
            max_pages: 2,
        };
        let res = c.search(&plan).await.unwrap();
        assert_eq!(res.hits.len(), 4, "dos páginas de 2 hits");
        assert!(!res.incomplete);
    }

    #[tokio::test]
    async fn search_respeta_el_tope_de_paginas() {
        let server = MockServer::start().await;
        // Siempre hay nextPageToken: sin tope pediría páginas infinitas.
        Mock::given(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_page(0, 2, Some("LOOP"))))
            .expect(1)
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let plan = SearchPlan {
            query: "x".into(),
            trending: false,
            max_pages: 1,
        };
        let res = c.search(&plan).await.unwrap();
        assert_eq!(res.hits.len(), 2, "una sola página por el tope");
        assert!(!res.incomplete);
    }

    #[tokio::test]
    async fn search_quota_exceeded_devuelve_parcial() {
        let server = MockServer::start().await;
        // Página 1 ok (token), página 2 -> quotaExceeded.
        Mock::given(path("/search"))
            .and(query_param_is_missing("pageToken"))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_page(0, 2, Some("P2"))))
            .mount(&server)
            .await;
        Mock::given(path("/search"))
            .and(query_param("pageToken", "P2"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": { "code": 403, "message": "quota",
                    "errors": [{ "reason": "quotaExceeded" }] }
            })))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let plan = SearchPlan {
            query: "x".into(),
            trending: false,
            max_pages: 5,
        };
        let res = c.search(&plan).await.unwrap();
        assert_eq!(res.hits.len(), 2, "conserva la página ya paga");
        assert!(res.incomplete, "debe marcar incompleto por cuota");
        assert!(res.incomplete_reason.is_some());
    }

    /// Una respuesta de `videos.list` con un único item de id `vid`.
    fn video_meta_page(vid: &str) -> serde_json::Value {
        serde_json::json!({
            "items": [{
                "id": vid,
                "snippet": {
                    "channelId": "UCcanal",
                    "title": "t",
                    "tags": [],
                    "publishedAt": "2021-09-27T03:00:00Z"
                },
                "statistics": { "viewCount": "10", "likeCount": "1", "commentCount": "0" }
            }]
        })
    }

    #[tokio::test]
    async fn fetch_video_meta_chunkea_de_a_50() {
        let server = MockServer::start().await;
        // 51 ids -> 2 requests (50 + 1). Cada request devuelve 1 item.
        Mock::given(path("/videos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(video_meta_page("v")))
            .expect(2)
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let ids: Vec<String> = (0..51).map(|i| format!("id{i}")).collect();
        let metas = c.fetch_video_meta(&ids).await.unwrap();
        // 1 item por request x 2 requests.
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].view_count, Some(10));
        // El server verifica en drop que se llamó exactamente 2 veces (chunking).
    }

    #[tokio::test]
    async fn fetch_video_meta_quota_exceeded_se_propaga() {
        let server = MockServer::start().await;
        Mock::given(path("/videos"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": { "code": 403, "message": "quota",
                    "errors": [{ "reason": "quotaExceeded" }] }
            })))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let err = c.fetch_video_meta(&["id0".to_string()]).await.unwrap_err();
        assert!(err.is_quota_exceeded());
    }

    #[tokio::test]
    async fn pagina_dos_paginas_y_junta_todo() {
        let server = MockServer::start().await;
        // Página 1 → token "P2"; página 2 → sin token (fin). La 1ra request va SIN
        // pageToken (el cliente real no manda params vacíos).
        Mock::given(path("/commentThreads"))
            .and(query_param_is_missing("pageToken"))
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

    #[tokio::test]
    async fn presupuesto_justo_al_fin_de_pagina_no_pide_otra_pagina() {
        // P15: la 1ra página trae EXACTAMENTE `budget` items y aún tiene
        // nextPageToken. El corte por presupuesto debe ocurrir ANTES de pedir la
        // 2da página (no gastar una unidad de cuota extra que se descartaría).
        let server = MockServer::start().await;
        // Mock SOLO de la 1ra página (sin pageToken), esperado exactamente 1 vez.
        // Si el cliente pidiera la 2da página (pageToken="P2") no habría mock que
        // matchee y el test fallaría por request sin respuesta.
        Mock::given(path("/commentThreads"))
            .and(query_param_is_missing("pageToken"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 3, Some("P2"))))
            .expect(1)
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let mut out = Vec::new();
        let reason = c
            .comments_for_video_into("vid1", &mut out, None, Some(3))
            .await
            .unwrap();
        assert_eq!(out.len(), 3, "trae justo el presupuesto");
        assert!(reason.unwrap().contains("comentarios"));
        // El drop del server verifica el `.expect(1)`: no se pidió la 2da página.
    }

    // --- ingest_channel_with: resiliencia de cuota F4 (auditoría P4) ----------

    #[tokio::test]
    async fn ingest_channel_saltea_comments_disabled_y_junta_el_resto() {
        let server = MockServer::start().await;
        // channels.list -> playlist de uploads.
        Mock::given(path("/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(channel_uploads("UP1")))
            .mount(&server)
            .await;
        // playlistItems -> dos videos.
        Mock::given(path("/playlistItems"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(playlist_page(&["vidA", "vidB"], None)),
            )
            .mount(&server)
            .await;
        // vidA: commentsDisabled (se saltea).
        Mock::given(path("/commentThreads"))
            .and(query_param("videoId", "vidA"))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(api_error_body("commentsDisabled")),
            )
            .mount(&server)
            .await;
        // vidB: 2 comentarios.
        Mock::given(path("/commentThreads"))
            .and(query_param("videoId", "vidB"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 2, None)))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let data = c
            .ingest_channel_with("UCx", IngestLimits::unlimited())
            .await
            .unwrap();
        assert_eq!(data.comments.len(), 2, "junta los de vidB, saltea vidA");
        assert!(!data.incomplete, "saltear no marca incompleto");
    }

    #[tokio::test]
    async fn ingest_channel_quota_exceeded_conserva_lo_traido() {
        let server = MockServer::start().await;
        Mock::given(path("/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(channel_uploads("UP1")))
            .mount(&server)
            .await;
        Mock::given(path("/playlistItems"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(playlist_page(&["vidA", "vidB"], None)),
            )
            .mount(&server)
            .await;
        // vidA: 2 comentarios ok.
        Mock::given(path("/commentThreads"))
            .and(query_param("videoId", "vidA"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 2, None)))
            .mount(&server)
            .await;
        // vidB: quotaExceeded -> corta pero conserva lo de vidA.
        Mock::given(path("/commentThreads"))
            .and(query_param("videoId", "vidB"))
            .respond_with(ResponseTemplate::new(403).set_body_json(api_error_body("quotaExceeded")))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let data = c
            .ingest_channel_with("UCx", IngestLimits::unlimited())
            .await
            .unwrap();
        assert_eq!(data.comments.len(), 2, "conserva lo ya pagado en cuota");
        assert!(data.incomplete, "cuota agotada marca incompleto");
        assert!(data.incomplete_reason.is_some());
    }

    #[tokio::test]
    async fn ingest_channel_respeta_max_videos() {
        let server = MockServer::start().await;
        Mock::given(path("/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(channel_uploads("UP1")))
            .mount(&server)
            .await;
        // La playlist tiene 3 videos pero max_videos=1 corta en el primero.
        Mock::given(path("/playlistItems"))
            .respond_with(ResponseTemplate::new(200).set_body_json(playlist_page(
                &["vidA", "vidB", "vidC"],
                Some("PP2"),
            )))
            .mount(&server)
            .await;
        Mock::given(path("/commentThreads"))
            .and(query_param("videoId", "vidA"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 1, None)))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let limits = IngestLimits {
            max_videos: Some(1),
            ..Default::default()
        };
        let data = c.ingest_channel_with("UCx", limits).await.unwrap();
        assert_eq!(data.comments.len(), 1, "solo el primer video");
        assert!(data.incomplete, "truncar por max_videos marca incompleto");
        assert!(data
            .incomplete_reason
            .unwrap()
            .contains("videos por canal"));
    }

    #[tokio::test]
    async fn ingest_channel_respeta_max_comments_global() {
        let server = MockServer::start().await;
        Mock::given(path("/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(channel_uploads("UP1")))
            .mount(&server)
            .await;
        Mock::given(path("/playlistItems"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(playlist_page(&["vidA", "vidB"], None)),
            )
            .mount(&server)
            .await;
        // vidA trae 3 comentarios; el presupuesto global es 3 -> al pasar a vidB ya
        // está alcanzado y corta.
        Mock::given(path("/commentThreads"))
            .and(query_param("videoId", "vidA"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 3, None)))
            .mount(&server)
            .await;

        let c = client_for(&server.uri());
        let limits = IngestLimits {
            max_comments: Some(3),
            ..Default::default()
        };
        let data = c.ingest_channel_with("UCx", limits).await.unwrap();
        assert_eq!(data.comments.len(), 3);
        assert!(data.incomplete);
        assert!(data.incomplete_reason.unwrap().contains("comentarios"));
    }
}
