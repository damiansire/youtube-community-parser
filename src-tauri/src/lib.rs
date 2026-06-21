//! App de escritorio (Tauri v2). Expone el dominio `sdp-core` al webview.
//!
//! Toda la lógica vive acá (no en `main.rs`) por compatibilidad con mobile.

mod youtube;

use std::path::PathBuf;

use sdp_core::{
    least_active_of, most_active_of, rank_commenters, Comment, Commenter, CommenterStats,
};
use sdp_storage::Store;
use secrecy::SecretString;
use serde::Serialize;
use tauri::Manager;
use youtube::{Ingested, IngestLimits, YoutubeClient};

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            analyze_demo,
            analyze_video,
            analyze_channel,
            analyze_history
        ])
        .run(tauri::generate_context!())
        .expect("error al iniciar la aplicación Tauri");
}
