//! App de escritorio (Tauri v2). Expone el dominio `sdp-core` al webview.
//!
//! Toda la lógica vive acá (no en `main.rs`) por compatibilidad con mobile.

mod youtube;

use std::collections::HashSet;
use std::path::PathBuf;

use sdp_core::{
    least_active_of, most_active_of, rank_commenters, AiIdea, AiProvider, BenchmarkReport, Comment,
    Commenter, CommenterStats, CorpusInsights, CostEstimate, CostPolicy, SearchPlan, SeoInput,
    SeoReport, VideoIdea, VideoMeta,
};
use sdp_storage::Store;
use secrecy::SecretString;
use serde::Serialize;
use tauri::Manager;
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

/// Ruta del archivo SQLite del histórico, dentro del directorio de datos de la
/// app (resuelto por Tauri, nunca hardcodeado).
fn db_path(app: &tauri::AppHandle) -> Result<PathBuf, CommandError> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| CommandError::DataDir(e.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::DataDir(e.to_string()))?;
    Ok(dir.join("history.sqlite3"))
}

/// Persiste una ingesta en el histórico local (F3: cablear `sdp-storage`).
///
/// rusqlite es **bloqueante**, así que el I/O corre en `spawn_blocking` para no
/// trabar el runtime async. Es idempotente (upsert por id / channel_id): re-
/// analizar el mismo canal no duplica, actualiza. Guardar el histórico permite
/// luego analizar la evolución **sin volver a gastar cuota** (`analyze_history`).
async fn persist(app: &tauri::AppHandle, data: &Ingested) -> Result<(), CommandError> {
    let path = db_path(app)?;
    let comments = data.comments.clone();
    let commenters = data.commenters.clone();
    tokio::task::spawn_blocking(move || -> Result<(), sdp_storage::StoreError> {
        let mut store = Store::open(path)?;
        store.save_commenters(&commenters)?;
        store.save_comments(&comments)?;
        Ok(())
    })
    .await
    .expect("la tarea de persistencia no debe paniquear")?;
    Ok(())
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
    let path = db_path(&app)?;
    let (comments, commenters) =
        tokio::task::spawn_blocking(move || -> Result<_, sdp_storage::StoreError> {
            let store = Store::open(path)?;
            Ok((store.all_comments()?, store.all_commenters()?))
        })
        .await
        .expect("la tarea de lectura del histórico no debe paniquear")?;
    Ok(Analysis::build(&comments, &commenters, 5))
}

/// Persiste metadata de videos en el histórico local (F9). Mismo patrón que
/// [`persist`]: rusqlite es bloqueante, va en `spawn_blocking`; upsert idempotente.
async fn persist_video_meta(
    app: &tauri::AppHandle,
    videos: &[VideoMeta],
) -> Result<(), CommandError> {
    let path = db_path(app)?;
    let videos = videos.to_vec();
    tokio::task::spawn_blocking(move || -> Result<(), sdp_storage::StoreError> {
        let mut store = Store::open(path)?;
        store.save_video_meta(&videos)?;
        Ok(())
    })
    .await
    .expect("la tarea de persistencia no debe paniquear")?;
    Ok(())
}

/// Gate de costo **server-side** (principio transversal, F6): si la operación
/// supera el umbral de la política y el usuario no confirmó, se rechaza ANTES de
/// gastar cuota. Re-calcula con el estimate recibido — no confía en el front.
fn ensure_confirmed(estimate: &CostEstimate, confirmed: bool) -> Result<(), CommandError> {
    if !confirmed && sdp_core::needs_optin(estimate, &CostPolicy::default()) {
        return Err(CommandError::NeedsConfirmation(format!(
            "{:?}; volvé a llamar con confirmed = true tras mostrar el modal",
            estimate.kind
        )));
    }
    Ok(())
}

/// Mina el **histórico local** (SQLite) para extraer keywords y temas recurrentes
/// de la comunidad (F6). Costo 0: trabaja sobre lo ya persistido, sin red.
#[tauri::command]
async fn analyze_corpus(app: tauri::AppHandle) -> Result<CorpusInsights, CommandError> {
    let path = db_path(&app)?;
    let comments = tokio::task::spawn_blocking(move || -> Result<_, sdp_storage::StoreError> {
        let store = Store::open(path)?;
        store.all_comments()
    })
    .await
    .expect("la tarea de lectura del histórico no debe paniquear")?;
    let texts: Vec<String> = comments.into_iter().map(|c| c.text).collect();
    Ok(sdp_core::corpus_insights(&texts))
}

/// Mina ideas de video desde el histórico local (F7): preguntas, pedidos y temas
/// recurrentes de la comunidad. Costo 0, sin red.
#[tauri::command]
async fn mine_ideas(app: tauri::AppHandle) -> Result<Vec<VideoIdea>, CommandError> {
    let path = db_path(&app)?;
    let comments = tokio::task::spawn_blocking(move || -> Result<_, sdp_storage::StoreError> {
        let store = Store::open(path)?;
        store.all_comments()
    })
    .await
    .expect("la tarea de lectura del histórico no debe paniquear")?;
    Ok(sdp_core::mine_video_ideas(&comments))
}

/// Mina las ideas del histórico local (compartido por F7 y la capa de IA F12).
async fn mined_ideas(app: &tauri::AppHandle) -> Result<Vec<sdp_core::VideoIdea>, CommandError> {
    let path = db_path(app)?;
    let comments = tokio::task::spawn_blocking(move || -> Result<_, sdp_storage::StoreError> {
        let store = Store::open(path)?;
        store.all_comments()
    })
    .await
    .expect("la tarea de lectura del histórico no debe paniquear")?;
    Ok(sdp_core::mine_video_ideas(&comments))
}

/// Estima el costo EN DINERO (US$) de refinar las ideas con IA (F12). Gratis: solo calcula.
#[tauri::command]
async fn estimate_ideas_ai(
    app: tauri::AppHandle,
    provider: AiProvider,
) -> Result<CostEstimate, CommandError> {
    let ideas = mined_ideas(&app).await?;
    Ok(sdp_core::estimate_ideas_ai(provider, &ideas))
}

/// Refina las ideas con IA (F12) TRAS pasar el gate de costo en US$. Inyecta el
/// adaptador concreto según `provider`; la key vive lo justo (SecretString).
#[tauri::command]
async fn refine_ideas_ai(
    app: tauri::AppHandle,
    provider: AiProvider,
    api_key: String,
    confirmed: bool,
) -> Result<Vec<AiIdea>, CommandError> {
    let ideas = mined_ideas(&app).await?;
    ensure_confirmed(&sdp_core::estimate_ideas_ai(provider, &ideas), confirmed)?;
    let prompt = sdp_core::build_ideas_prompt(&ideas);
    let client = sdp_llm::build_provider(provider, SecretString::from(api_key), None)?;
    let raw = client.enhance(&prompt).await?;
    Ok(sdp_core::parse_ideas_response(&raw)?)
}

/// Audita el SEO de un texto candidato (F8) cruzándolo con las keywords que
/// demanda la comunidad (del histórico local). Costo 0, sin red.
#[tauri::command]
async fn audit_seo(app: tauri::AppHandle, input: SeoInput) -> Result<SeoReport, CommandError> {
    let path = db_path(&app)?;
    let comments = tokio::task::spawn_blocking(move || -> Result<_, sdp_storage::StoreError> {
        let store = Store::open(path)?;
        store.all_comments()
    })
    .await
    .expect("la tarea de lectura del histórico no debe paniquear")?;
    let texts: Vec<String> = comments.into_iter().map(|c| c.text).collect();
    let corpus = sdp_core::corpus_insights(&texts);
    Ok(sdp_core::audit_seo(&input, &corpus))
}

/// Estima el costo de traer la metadata de `ids` videos (F9). Gratis: solo
/// calcula (`ceil(n/50)` unidades). La UI lo muestra antes de `fetch_video_meta`.
#[tauri::command]
fn estimate_video_meta(ids: Vec<String>) -> CostEstimate {
    sdp_core::cost::estimate_video_meta(ids.len())
}

/// Trae la metadata de videos (F9) **tras pasar el gate de costo** y la persiste.
/// Re-estima server-side; sin `confirmed` cuando hay costo, no ejecuta.
#[tauri::command]
async fn fetch_video_meta(
    app: tauri::AppHandle,
    ids: Vec<String>,
    api_key: String,
    confirmed: bool,
) -> Result<Vec<VideoMeta>, CommandError> {
    ensure_confirmed(&sdp_core::cost::estimate_video_meta(ids.len()), confirmed)?;
    let videos = client(api_key)?.fetch_video_meta(&ids).await?;
    persist_video_meta(&app, &videos).await?;
    Ok(videos)
}

/// Estima el costo de una búsqueda (F10): 100 unidades por página. Gratis: solo
/// calcula. `requires_confirmation` siempre es `true` para cualquier página.
#[tauri::command]
fn estimate_search(plan: SearchPlan) -> CostEstimate {
    sdp_core::cost::estimate_search(plan.max_pages)
}

/// Ejecuta una búsqueda/trending (F10) **tras pasar el gate de costo** (cara:
/// 100u/página). Devuelve resultados parciales con `incomplete` si la cuota se
/// agota a mitad (F4).
#[tauri::command]
async fn run_search(
    plan: SearchPlan,
    api_key: String,
    confirmed: bool,
) -> Result<SearchResults, CommandError> {
    ensure_confirmed(&sdp_core::cost::estimate_search(plan.max_pages), confirmed)?;
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
    let path = db_path(&app)?;
    let (videos, comments) =
        tokio::task::spawn_blocking(move || -> Result<_, sdp_storage::StoreError> {
            let store = Store::open(path)?;
            Ok((store.all_video_meta()?, store.all_comments()?))
        })
        .await
        .expect("la tarea de lectura del histórico no debe paniquear")?;

    // Perfila un canal: sus videos + los comentarios sobre esos videos.
    let profile = |id: &str| {
        let chan_videos: Vec<VideoMeta> = videos
            .iter()
            .filter(|v| v.channel_id == id)
            .cloned()
            .collect();
        let video_ids: HashSet<&str> = chan_videos.iter().map(|v| v.video_id.as_str()).collect();
        let chan_comments: Vec<Comment> = comments
            .iter()
            .filter(|c| video_ids.contains(c.video_id.as_str()))
            .cloned()
            .collect();
        sdp_core::profile_channel(id, &chan_videos, &chan_comments)
    };

    let mine = profile(&my_id);
    let competitors = competitor_ids.iter().map(|id| profile(id)).collect();
    Ok(sdp_core::benchmark(mine, competitors))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
