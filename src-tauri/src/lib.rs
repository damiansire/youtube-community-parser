//! App de escritorio (Tauri v2). Expone el dominio `sdp-core` al webview.
//!
//! Toda la lógica vive acá (no en `main.rs`) por compatibilidad con mobile.

mod confirm;
mod youtube;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use confirm::{ConfirmError, ConfirmStore, OpFingerprint};
use sdp_core::{
    least_active_of, most_active_of, rank_commenters, AiIdea, AiProvider, BenchmarkReport, Comment,
    Commenter, CommenterStats, CorpusInsights, CostEstimate, CostPolicy, SearchPlan, SeoInput,
    SeoReport, VideoIdea, VideoMeta,
};
use sdp_storage::Store;
use secrecy::SecretString;
use serde::Serialize;
use tauri::{Manager, State};
use youtube::{IngestLimits, Ingested, SearchResults, YoutubeClient};

/// Tope por defecto de la ingesta de un canal (F4): evita agotar la cuota
/// diaria (10k u/día) en un canal enorme. Un canal mediano cabe holgado; si se
/// excede, se devuelve lo parcial con `incomplete = true`. Conservador a
/// propósito — la cuota es el cuello real, no la memoria.
const DEFAULT_CHANNEL_LIMITS: IngestLimits = IngestLimits {
    max_videos: Some(200),
    max_comments: Some(20_000),
    max_pages_per_video: Some(50),
};

/// Respuesta de un análisis: ranking completo + los extremos ya recortados,
/// listo para que la UI arme sus tablas sin recalcular.
#[derive(Debug, Serialize)]
pub struct Analysis {
    pub total_comments: usize,
    pub total_commenters: usize,
    pub ranking: Vec<CommenterStats>,
    pub top: Vec<CommenterStats>,
    pub bottom: Vec<CommenterStats>,
    /// `true` si la ingesta se cortó a mitad (típicamente por cuota): los datos
    /// son parciales pero válidos (F4). La UI puede avisar al usuario.
    pub incomplete: bool,
    /// Motivo legible del corte, si lo hubo.
    pub incomplete_reason: Option<String>,
}

impl Analysis {
    /// Construye el análisis a partir de una ingesta (preservando su flag de
    /// incompletitud).
    fn from_ingested(data: &Ingested, extremes: usize) -> Self {
        let mut analysis = Self::build(&data.comments, &data.commenters, extremes);
        analysis.incomplete = data.incomplete;
        analysis.incomplete_reason = data.incomplete_reason.clone();
        analysis
    }

    fn build(comments: &[Comment], commenters: &[Commenter], extremes: usize) -> Self {
        // F5: calculamos el ranking UNA vez y derivamos ambos extremos de él, en
        // lugar de recalcularlo por cada extremo.
        let ranking = rank_commenters(comments, commenters);
        let top = most_active_of(&ranking, extremes);
        // El tramo inferior (cola del mismo ranking) se solapa con `top` cuando
        // hay <= 2*extremes comentaristas (la misma persona saldría como "de las
        // que más" y "de las que menos" a la vez). Excluimos del tramo inferior a
        // quienes ya están en el tope.
        let top_ids: std::collections::HashSet<&str> =
            top.iter().map(|s| s.channel_id.as_str()).collect();
        let bottom = least_active_of(&ranking, extremes)
            .into_iter()
            .filter(|s| !top_ids.contains(s.channel_id.as_str()))
            .collect();

        Analysis {
            total_comments: comments.len(),
            total_commenters: commenters.len(),
            ranking,
            top,
            bottom,
            incomplete: false,
            incomplete_reason: None,
        }
    }
}

/// Error de comando, serializable para cruzar el límite IPC.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Youtube(#[from] youtube::YoutubeError),
    #[error("error de persistencia local: {0}")]
    Storage(#[from] sdp_storage::StoreError),
    #[error("no se pudo resolver el directorio de datos de la app: {0}")]
    DataDir(String),
    #[error("esta operación requiere confirmación de costo: {0}")]
    NeedsConfirmation(String),
    #[error("error de la capa de IA: {0}")]
    Llm(#[from] sdp_llm::LlmError),
    #[error("no se pudo parsear la respuesta de IA: {0}")]
    AiParse(#[from] sdp_core::ParseError),
    #[error("una tarea interna falló inesperadamente: {0}")]
    Task(String),
}

impl From<ConfirmError> for CommandError {
    fn from(e: ConfirmError) -> Self {
        let msg = match e {
            ConfirmError::Missing => {
                "falta el token de confirmación; estimá la operación y confirmá el modal"
            }
            ConfirmError::Unknown => {
                "el token de confirmación no es válido (ya se usó o expiró); volvé a estimar"
            }
            ConfirmError::Mismatch => {
                "los datos cambiaron desde que confirmaste; volvé a estimar y confirmá de nuevo"
            }
        };
        CommandError::NeedsConfirmation(msg.to_string())
    }
}

/// Helper para mapear un `JoinError` de `spawn_blocking` a un `CommandError`
/// tipado en lugar de paniquear (un panic en la tarea crasheaba el comando).
fn join_err(e: tokio::task::JoinError) -> CommandError {
    CommandError::Task(e.to_string())
}

/// Estimación + token de confirmación de un solo uso (auditoría P1). El front
/// muestra el modal con `estimate` y, al confirmar, devuelve `confirmation_token`
/// al `run_*` correspondiente. `None` cuando la operación es gratis.
#[derive(Debug, Serialize)]
pub struct ConfirmableEstimate {
    #[serde(flatten)]
    pub estimate: CostEstimate,
    pub confirmation_token: Option<String>,
}

/// Construye la respuesta de un `estimate_*`: si la operación requiere
/// confirmación, emite un token ligado al fingerprint; si es gratis, no.
fn confirmable(
    store: &ConfirmStore,
    op: &str,
    estimate: CostEstimate,
    corpus_hash: u64,
) -> ConfirmableEstimate {
    let confirmation_token = if estimate.requires_confirmation {
        Some(store.issue(OpFingerprint::new(op, &estimate, corpus_hash)))
    } else {
        None
    };
    ConfirmableEstimate {
        estimate,
        confirmation_token,
    }
}

/// Valida (y consume) el token de confirmación contra el estimate re-calculado
/// server-side. Si la operación NO requiere confirmación, pasa sin token. Cierra
/// el agujero del `confirmed: bool` crudo (auditoría P1).
fn ensure_confirmed_token(
    store: &ConfirmStore,
    op: &str,
    estimate: &CostEstimate,
    corpus_hash: u64,
    token: Option<&str>,
) -> Result<(), CommandError> {
    if !sdp_core::needs_optin(estimate, &CostPolicy::default()) {
        return Ok(());
    }
    let expected = OpFingerprint::new(op, estimate, corpus_hash);
    store.consume(token, &expected)?;
    Ok(())
}

impl serde::Serialize for CommandError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// Datos de muestra para ver la app funcionando sin API key.
fn sample() -> (Vec<Comment>, Vec<Commenter>) {
    use chrono::{TimeZone, Utc};
    let at = |h: u32| Utc.with_ymd_and_hms(2021, 9, 27, h, 0, 0).single().unwrap();
    let commenters = vec![
        Commenter {
            channel_id: "ana".into(),
            display_name: "Ana".into(),
            profile_image_url: None,
            channel_url: None,
        },
        Commenter {
            channel_id: "beto".into(),
            display_name: "Beto".into(),
            profile_image_url: None,
            channel_url: None,
        },
        Commenter {
            channel_id: "caro".into(),
            display_name: "Caro".into(),
            profile_image_url: None,
            channel_url: None,
        },
        Commenter {
            channel_id: "dia".into(),
            display_name: "Día".into(),
            profile_image_url: None,
            channel_url: None,
        },
    ];
    let c = |id: &str, who: &str, likes: u64, h: u32| Comment {
        id: id.into(),
        video_id: "demo".into(),
        author_channel_id: who.into(),
        text: "comentario de ejemplo".into(),
        like_count: likes,
        published_at: at(h),
    };
    let comments = vec![
        c("1", "ana", 5, 9),
        c("2", "ana", 2, 10),
        c("3", "ana", 0, 11),
        c("4", "ana", 1, 12),
        c("5", "beto", 8, 9),
        c("6", "beto", 3, 13),
        c("7", "caro", 1, 14),
        c("8", "caro", 0, 15),
        c("9", "dia", 12, 10),
    ];
    (comments, commenters)
}

/// Análisis de demostración (sin red): permite ver la UI ya.
#[tauri::command]
fn analyze_demo() -> Analysis {
    let (comments, commenters) = sample();
    Analysis::build(&comments, &commenters, 3)
}

/// Envuelve la API key en `SecretString` en cuanto cruza el IPC, para
/// minimizar el tiempo que vive en claro en memoria (F2). Ver `youtube.rs`.
fn client(api_key: String) -> Result<YoutubeClient, CommandError> {
    Ok(YoutubeClient::new(SecretString::from(api_key))?)
}

/// Conexión SQLite **compartida y reutilizable** (auditoría P9), guardada en
/// `tauri::State`. Antes cada comando hacía `Store::open(path)`, re-ejecutando el
/// DDL (`CREATE TABLE/INDEX IF NOT EXISTS`) en cada apertura y sin reusar la
/// conexión. Ahora se abre **una sola vez** al arranque (DDL + PRAGMAs WAL una
/// vez) y cada comando la reusa.
///
/// rusqlite es bloqueante y `Connection` no es `Sync`, así que se envuelve en
/// `Arc<Mutex<Store>>`: el `Arc` se clona barato para cruzar a `spawn_blocking`
/// (la rama bloqueante, fuera del executor de Tokio) y ahí se toma el lock.
#[derive(Clone)]
pub struct Db(Arc<Mutex<Store>>);

impl Db {
    /// Abre la base en `path` una vez (corre DDL + PRAGMAs) y la deja lista para
    /// compartir. Se llama en el `setup` de Tauri, no por comando.
    fn open(path: PathBuf) -> Result<Self, sdp_storage::StoreError> {
        Ok(Db(Arc::new(Mutex::new(Store::open(path)?))))
    }

    /// Clona el handle compartido (solo el `Arc`, no la conexión).
    fn handle(&self) -> Arc<Mutex<Store>> {
        Arc::clone(&self.0)
    }
}

/// Ejecuta `f` sobre el `Store` compartido dentro de `spawn_blocking` (rusqlite
/// es bloqueante). Toma el handle del `State`, lo mueve a la tarea y ahí toma el
/// lock. Centraliza el manejo del `JoinError` (tipado, no panic) y del `PoisonError`
/// del mutex (auditoría P9/P10).
async fn with_store<T, F>(app: &tauri::AppHandle, f: F) -> Result<T, CommandError>
where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> Result<T, sdp_storage::StoreError> + Send + 'static,
{
    let db = app.state::<Db>().handle();
    tokio::task::spawn_blocking(move || -> Result<T, CommandError> {
        let mut store = db
            .lock()
            .map_err(|_| CommandError::Task("la conexión a la base quedó envenenada".into()))?;
        Ok(f(&mut store)?)
    })
    .await
    .map_err(join_err)?
}

/// Lee todos los comentarios del histórico local (SQLite). Encapsula el patrón
/// que estaba copiado en 6 comandos, reusando la conexión compartida (auditoría
/// P9/P10) y mapeando el `JoinError` a un `CommandError` tipado en vez de paniquear.
async fn read_comments(app: &tauri::AppHandle) -> Result<Vec<Comment>, CommandError> {
    with_store(app, |store| store.all_comments()).await
}

/// Persiste una ingesta en el histórico local (F3: cablear `sdp-storage`).
///
/// rusqlite es **bloqueante**, así que el I/O corre en `spawn_blocking` para no
/// trabar el runtime async. Es idempotente (upsert por id / channel_id): re-
/// analizar el mismo canal no duplica, actualiza. Guardar el histórico permite
/// luego analizar la evolución **sin volver a gastar cuota** (`analyze_history`).
async fn persist(app: &tauri::AppHandle, data: &Ingested) -> Result<(), CommandError> {
    let comments = data.comments.clone();
    let commenters = data.commenters.clone();
    with_store(app, move |store| {
        store.save_commenters(&commenters)?;
        store.save_comments(&comments)?;
        Ok(())
    })
    .await
}

/// Analiza los comentarios de un video real vía el cliente nativo de YouTube.
///
/// La ingesta es **async pura** (reqwest): no hay subproceso Node ni
/// `spawn_blocking`. Corre directo en el runtime de Tokio de Tauri. Lo ingerido
/// se persiste en el histórico local (F3).
#[tauri::command]
async fn analyze_video(
    app: tauri::AppHandle,
    video_id: String,
    api_key: String,
) -> Result<Analysis, CommandError> {
    let data = client(api_key)?.ingest_video(&video_id).await?;
    persist(&app, &data).await?;
    Ok(Analysis::from_ingested(&data, 5))
}

/// Analiza todos los comentarios de un canal real vía el cliente nativo.
///
/// Igual que `analyze_video`, pero con **topes de cuota** (F4): recorre hasta
/// `DEFAULT_CHANNEL_LIMITS` y, si los alcanza o si la API tira `quotaExceeded` a
/// mitad, devuelve lo parcial con `incomplete = true` en vez de descartar todo.
/// Lo ingerido (parcial o completo) se persiste en el histórico local (F3).
#[tauri::command]
async fn analyze_channel(
    app: tauri::AppHandle,
    channel_id: String,
    api_key: String,
) -> Result<Analysis, CommandError> {
    let data = client(api_key)?
        .ingest_channel_with(&channel_id, DEFAULT_CHANNEL_LIMITS)
        .await?;
    persist(&app, &data).await?;
    Ok(Analysis::from_ingested(&data, 5))
}

/// Analiza el **histórico local** acumulado (SQLite), **sin pegarle a la API**
/// ni gastar cuota (F3). Es la razón de ser de `sdp-storage`: ver la evolución
/// de la comunidad reusando lo ya ingerido en sesiones anteriores.
#[tauri::command]
async fn analyze_history(app: tauri::AppHandle) -> Result<Analysis, CommandError> {
    let (comments, commenters) = with_store(&app, |store| {
        Ok((store.all_comments()?, store.all_commenters()?))
    })
    .await?;
    Ok(Analysis::build(&comments, &commenters, 5))
}

/// Persiste metadata de videos en el histórico local (F9). Mismo patrón que
/// [`persist`]: rusqlite es bloqueante, va en `spawn_blocking`; upsert idempotente.
async fn persist_video_meta(
    app: &tauri::AppHandle,
    videos: &[VideoMeta],
) -> Result<(), CommandError> {
    let videos = videos.to_vec();
    with_store(app, move |store| store.save_video_meta(&videos)).await
}

/// Mina el **histórico local** (SQLite) para extraer keywords y temas recurrentes
/// de la comunidad (F6). Costo 0: trabaja sobre lo ya persistido, sin red.
#[tauri::command]
async fn analyze_corpus(app: tauri::AppHandle) -> Result<CorpusInsights, CommandError> {
    let comments = read_comments(&app).await?;
    let texts: Vec<String> = comments.into_iter().map(|c| c.text).collect();
    Ok(sdp_core::corpus_insights(&texts))
}

/// Mina las ideas del histórico local (compartido por F7 y la capa de IA F12).
/// Única fuente de verdad: el comando `mine_ideas` delega acá para que el
/// criterio de minería que ve el usuario coincida con el que se estima y cobra
/// (auditoría P10).
async fn mined_ideas(app: &tauri::AppHandle) -> Result<Vec<sdp_core::VideoIdea>, CommandError> {
    let comments = read_comments(app).await?;
    Ok(sdp_core::mine_video_ideas(&comments))
}

/// Mina ideas de video desde el histórico local (F7): preguntas, pedidos y temas
/// recurrentes de la comunidad. Costo 0, sin red.
#[tauri::command]
async fn mine_ideas(app: tauri::AppHandle) -> Result<Vec<VideoIdea>, CommandError> {
    mined_ideas(&app).await
}

/// Estima el costo EN DINERO (US$) de refinar las ideas con IA (F12). Gratis:
/// solo calcula. Emite un **token de confirmación** ligado al monto y al hash de
/// las ideas minadas (auditoría P1): `refine_ideas_ai` lo exige de vuelta.
#[tauri::command]
async fn estimate_ideas_ai(
    app: tauri::AppHandle,
    provider: AiProvider,
    confirm: State<'_, ConfirmStore>,
) -> Result<ConfirmableEstimate, CommandError> {
    let ideas = mined_ideas(&app).await?;
    let estimate = sdp_core::estimate_ideas_ai(provider, &ideas);
    let corpus_hash = confirm::hash_texts(&ideas_fingerprint(&ideas));
    let op = format!("refine_ideas_ai:{provider:?}");
    Ok(confirmable(&confirm, &op, estimate, corpus_hash))
}

/// Huella de las ideas para ligar el token al insumo exacto (cambia si cambian
/// las ideas → el monto re-estimado también cambia, cerrando el TOCTOU).
fn ideas_fingerprint(ideas: &[VideoIdea]) -> Vec<String> {
    ideas
        .iter()
        .map(|i| {
            format!(
                "{}|{:?}|{}",
                i.title_seed,
                i.signal,
                i.supporting_comment_ids.len()
            )
        })
        .collect()
}

/// Refina las ideas con IA (F12) TRAS pasar el gate de costo en US$, validado por
/// un **token de un solo uso** (auditoría P1) ligado al monto y al corpus minado.
/// El `max_tokens` se deriva del mismo presupuesto del estimate para no truncar
/// la respuesta y cobrar por un JSON inválido (auditoría P2).
#[tauri::command]
async fn refine_ideas_ai(
    app: tauri::AppHandle,
    provider: AiProvider,
    api_key: String,
    confirmation_token: Option<String>,
    confirm: State<'_, ConfirmStore>,
) -> Result<Vec<AiIdea>, CommandError> {
    let ideas = mined_ideas(&app).await?;
    let estimate = sdp_core::estimate_ideas_ai(provider, &ideas);
    let corpus_hash = confirm::hash_texts(&ideas_fingerprint(&ideas));
    let op = format!("refine_ideas_ai:{provider:?}");
    ensure_confirmed_token(
        &confirm,
        &op,
        &estimate,
        corpus_hash,
        confirmation_token.as_deref(),
    )?;

    let prompt = sdp_core::build_ideas_prompt(&ideas);
    // Tope de salida derivado del presupuesto: lo ejecutado == lo estimado.
    let max_tokens = sdp_core::max_output_tokens_for(ideas.len()).min(u32::MAX as u64) as u32;
    let client = sdp_llm::build_provider(
        provider,
        SecretString::from(api_key),
        None,
        Some(max_tokens),
    )?;
    let raw = client.enhance(&prompt).await?;
    sdp_core::parse_ideas_response(&raw).map_err(CommandError::from)
}

/// Audita el SEO de un texto candidato (F8) cruzándolo con las keywords que
/// demanda la comunidad (del histórico local). Costo 0, sin red.
#[tauri::command]
async fn audit_seo(app: tauri::AppHandle, input: SeoInput) -> Result<SeoReport, CommandError> {
    let comments = read_comments(&app).await?;
    let texts: Vec<String> = comments.into_iter().map(|c| c.text).collect();
    let corpus = sdp_core::corpus_insights(&texts);
    Ok(sdp_core::audit_seo(&input, &corpus))
}

/// Estima el costo de traer la metadata de `ids` videos (F9). Gratis: solo
/// calcula (`ceil(n/50)` unidades). Emite un token de confirmación ligado al
/// conjunto de ids (auditoría P1).
#[tauri::command]
fn estimate_video_meta(ids: Vec<String>, confirm: State<'_, ConfirmStore>) -> ConfirmableEstimate {
    let estimate = sdp_core::cost::estimate_video_meta(ids.len());
    let corpus_hash = confirm::hash_texts(&ids);
    confirmable(&confirm, "fetch_video_meta", estimate, corpus_hash)
}

/// Resultado de `fetch_video_meta` (F9): los videos traídos y los IDs pedidos que
/// la API NO devolvió (inexistentes/privados). El front reconcilia la diferencia
/// en vez de mostrar `videos.length` a secas (auditoría P12), alineándose con el
/// patrón `incomplete`/`incomplete_reason` de los comandos hermanos.
#[derive(Debug, Serialize)]
pub struct VideoMetaResult {
    pub videos: Vec<VideoMeta>,
    pub missing_ids: Vec<String>,
}

/// Trae la metadata de videos (F9) **tras pasar el gate de costo** (token de un
/// solo uso, auditoría P1) y la persiste. Re-estima server-side.
#[tauri::command]
async fn fetch_video_meta(
    app: tauri::AppHandle,
    ids: Vec<String>,
    api_key: String,
    confirmation_token: Option<String>,
    confirm: State<'_, ConfirmStore>,
) -> Result<VideoMetaResult, CommandError> {
    let estimate = sdp_core::cost::estimate_video_meta(ids.len());
    let corpus_hash = confirm::hash_texts(&ids);
    ensure_confirmed_token(
        &confirm,
        "fetch_video_meta",
        &estimate,
        corpus_hash,
        confirmation_token.as_deref(),
    )?;
    let videos = client(api_key)?.fetch_video_meta(&ids).await?;
    persist_video_meta(&app, &videos).await?;
    let missing_ids = missing_video_ids(&ids, &videos);
    Ok(VideoMetaResult {
        videos,
        missing_ids,
    })
}

/// IDs pedidos que no volvieron en la respuesta (la API omite los inexistentes).
/// Deduplica y preserva el orden de aparición. PURO — testeable sin red.
fn missing_video_ids(requested: &[String], returned: &[VideoMeta]) -> Vec<String> {
    let present: HashSet<&str> = returned.iter().map(|v| v.video_id.as_str()).collect();
    let mut seen = HashSet::new();
    requested
        .iter()
        .filter(|id| !present.contains(id.as_str()))
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect()
}

/// Estima el costo de una búsqueda (F10): 100 unidades por página. Gratis: solo
/// calcula. Emite un token de confirmación ligado al plan (auditoría P1).
#[tauri::command]
fn estimate_search(plan: SearchPlan, confirm: State<'_, ConfirmStore>) -> ConfirmableEstimate {
    let estimate = sdp_core::cost::estimate_search(plan.max_pages);
    let corpus_hash = confirm::hash_texts(&[search_fingerprint(&plan)]);
    confirmable(&confirm, "run_search", estimate, corpus_hash)
}

/// Huella del plan de búsqueda para ligar el token: si cambia query/trending/
/// páginas entre estimar y ejecutar, el token deja de servir.
fn search_fingerprint(plan: &SearchPlan) -> String {
    format!("{}|{}|{}", plan.query, plan.trending, plan.max_pages)
}

/// Ejecuta una búsqueda/trending (F10) **tras pasar el gate de costo** (cara:
/// 100u/página) validado por un token de un solo uso (auditoría P1). Devuelve
/// resultados parciales con `incomplete` si la cuota se agota a mitad (F4).
#[tauri::command]
async fn run_search(
    plan: SearchPlan,
    api_key: String,
    confirmation_token: Option<String>,
    confirm: State<'_, ConfirmStore>,
) -> Result<SearchResults, CommandError> {
    let estimate = sdp_core::cost::estimate_search(plan.max_pages);
    let corpus_hash = confirm::hash_texts(&[search_fingerprint(&plan)]);
    ensure_confirmed_token(
        &confirm,
        "run_search",
        &estimate,
        corpus_hash,
        confirmation_token.as_deref(),
    )?;
    Ok(client(api_key)?.search(&plan).await?)
}

/// Compara mi canal contra competidores (F11) usando la metadata de videos ya
/// ingestada (F9) y los comentarios. Costo 0, sin red. Un competidor sin datos
/// aparece como brecha "sin datos" en vez de fallar.
#[tauri::command]
async fn benchmark_channels(
    app: tauri::AppHandle,
    my_id: String,
    competitor_ids: Vec<String>,
) -> Result<BenchmarkReport, CommandError> {
    let (videos, comments) = with_store(&app, |store| {
        Ok((store.all_video_meta()?, store.all_comments()?))
    })
    .await?;

    // Agrupamos UNA sola vez en O(V+C) en lugar de re-escanear y clonar TODO el
    // histórico por cada canal (era O((1+M)·(V+C)) con clones completos, P7):
    //   - videos por channel_id;
    //   - comentarios por el channel_id dueño de su video (vía video_id→channel).
    use std::collections::HashMap;
    let mut videos_by_channel: HashMap<&str, Vec<&VideoMeta>> = HashMap::new();
    let mut channel_of_video: HashMap<&str, &str> = HashMap::new();
    for v in &videos {
        videos_by_channel
            .entry(v.channel_id.as_str())
            .or_default()
            .push(v);
        channel_of_video.insert(v.video_id.as_str(), v.channel_id.as_str());
    }
    let mut comments_by_channel: HashMap<&str, Vec<&Comment>> = HashMap::new();
    for c in &comments {
        if let Some(chan) = channel_of_video.get(c.video_id.as_str()) {
            comments_by_channel.entry(chan).or_default().push(c);
        }
    }

    // Perfila un canal clonando SOLO sus propios items (no el histórico entero).
    let profile = |id: &str| {
        let chan_videos: Vec<VideoMeta> = videos_by_channel
            .get(id)
            .map(|vs| vs.iter().map(|v| (*v).clone()).collect())
            .unwrap_or_default();
        let chan_comments: Vec<Comment> = comments_by_channel
            .get(id)
            .map(|cs| cs.iter().map(|c| (*c).clone()).collect())
            .unwrap_or_default();
        sdp_core::profile_channel(id, &chan_videos, &chan_comments)
    };

    let mine = profile(&my_id);
    let competitors = competitor_ids.iter().map(|id| profile(id)).collect();
    Ok(sdp_core::benchmark(mine, competitors))
}

/// Resuelve la ruta del SQLite y abre la conexión compartida UNA vez al arranque
/// (auditoría P9). El `create_dir_all` bloqueante acá es aceptable: corre una sola
/// vez en el `setup`, no por comando dentro del executor de Tokio.
fn init_db(app: &tauri::AppHandle) -> Result<Db, Box<dyn std::error::Error>> {
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(Db::open(dir.join("history.sqlite3"))?)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ConfirmStore::new())
        .setup(|app| {
            // Conexión SQLite compartida (P9): se abre una vez y se guarda en State.
            let db = init_db(app.handle())?;
            app.manage(db);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            analyze_demo,
            analyze_video,
            analyze_channel,
            analyze_history,
            analyze_corpus,
            mine_ideas,
            estimate_ideas_ai,
            refine_ideas_ai,
            audit_seo,
            estimate_video_meta,
            fetch_video_meta,
            estimate_search,
            run_search,
            benchmark_channels
        ])
        .run(tauri::generate_context!())
        .expect("error al iniciar la aplicación Tauri");
}

#[cfg(test)]
mod tests {
    //! Tests de la capa de comandos (auditoría P5). Cubren las piezas **puras**
    //! que no necesitan un `AppHandle`: el armado del análisis (sin solapamiento
    //! entre extremos), el gate de confirmación por token (free/gated/replay/
    //! mismatch) y el cálculo de ids faltantes. La verificación de que el gate
    //! corre ANTES de pegarle al proveedor está cubierta estructuralmente: el
    //! token se consume antes de `build_provider` y `confirm.rs` testea el gate.
    use super::*;

    #[test]
    fn analysis_build_no_solapa_top_y_bottom() {
        // Con pocos comentaristas y extremes alto, top y bottom se solaparían si
        // no excluyéramos: el contrato es top ∩ bottom = ∅.
        let (comments, commenters) = sample(); // 4 comentaristas
        let a = Analysis::build(&comments, &commenters, 3);
        let top_ids: HashSet<&str> = a.top.iter().map(|s| s.channel_id.as_str()).collect();
        let bottom_ids: HashSet<&str> = a.bottom.iter().map(|s| s.channel_id.as_str()).collect();
        assert!(
            top_ids.is_disjoint(&bottom_ids),
            "ningún comentarista puede estar en top y bottom a la vez"
        );
        assert_eq!(a.total_commenters, 4);
        // Cada persona aparece a lo sumo una vez entre ambos extremos.
        assert!(a.top.len() + a.bottom.len() <= a.total_commenters);
    }

    #[test]
    fn gate_token_operacion_gratis_no_exige_token() {
        // estimate_search(0) = 0 unidades => gratis => pasa sin token.
        let store = ConfirmStore::new();
        let free = sdp_core::cost::estimate_search(0);
        assert!(!free.requires_confirmation);
        let r = ensure_confirmed_token(&store, "run_search", &free, 0, None);
        assert!(
            r.is_ok(),
            "una operación gratis no debe exigir confirmación"
        );
    }

    #[test]
    fn gate_token_operacion_cara_sin_token_falla() {
        // estimate_search(1) = 100 unidades => requiere confirmación.
        let store = ConfirmStore::new();
        let gated = sdp_core::cost::estimate_search(1);
        assert!(gated.requires_confirmation);
        let r = ensure_confirmed_token(&store, "run_search", &gated, 7, None);
        assert!(
            matches!(r, Err(CommandError::NeedsConfirmation(_))),
            "operación cara sin token debe rechazarse"
        );
    }

    #[test]
    fn gate_token_emitido_y_consumido_una_sola_vez() {
        let store = ConfirmStore::new();
        let gated = sdp_core::cost::estimate_search(1);
        // estimate_* emite el token...
        let issued = confirmable(&store, "run_search", gated.clone(), 7);
        let token = issued.confirmation_token.expect("debe emitir token");
        // ...run_* lo consume con el MISMO fingerprint: pasa.
        assert!(ensure_confirmed_token(&store, "run_search", &gated, 7, Some(&token)).is_ok());
        // Replay del mismo token: ya consumido => falla.
        assert!(matches!(
            ensure_confirmed_token(&store, "run_search", &gated, 7, Some(&token)),
            Err(CommandError::NeedsConfirmation(_))
        ));
    }

    #[test]
    fn gate_token_con_corpus_distinto_no_sirve() {
        // Confirmó sobre un corpus_hash y al ejecutar cambió (TOCTOU): rechazo.
        let store = ConfirmStore::new();
        let gated = sdp_core::cost::estimate_search(1);
        let issued = confirmable(&store, "run_search", gated.clone(), 100);
        let token = issued.confirmation_token.unwrap();
        let r = ensure_confirmed_token(&store, "run_search", &gated, 999, Some(&token));
        assert!(matches!(r, Err(CommandError::NeedsConfirmation(_))));
    }

    #[test]
    fn missing_video_ids_deduplica_y_preserva_orden() {
        let requested = vec![
            "a".to_string(),
            "b".to_string(),
            "a".to_string(),
            "c".to_string(),
        ];
        let returned = vec![VideoMeta {
            video_id: "b".into(),
            channel_id: "ch".into(),
            title: "t".into(),
            description: String::new(),
            tags: vec![],
            view_count: None,
            like_count: None,
            comment_count: None,
            published_at: chrono::Utc::now(),
        }];
        // "b" volvió; faltan "a" (deduplicado) y "c", en orden de aparición.
        assert_eq!(missing_video_ids(&requested, &returned), vec!["a", "c"]);
    }
}
