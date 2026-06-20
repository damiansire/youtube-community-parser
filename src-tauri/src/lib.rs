//! App de escritorio (Tauri v2). Expone el dominio `sdp-core` al webview.
//!
//! Toda la lógica vive acá (no en `main.rs`) por compatibilidad con mobile.

mod ingest;

use sdp_core::{least_active, most_active, rank_commenters, Comment, Commenter, CommenterStats};
use serde::Serialize;

/// Respuesta de un análisis: ranking completo + los extremos ya recortados,
/// listo para que la UI arme sus tablas sin recalcular.
#[derive(Debug, Serialize)]
pub struct Analysis {
    pub total_comments: usize,
    pub total_commenters: usize,
    pub ranking: Vec<CommenterStats>,
    pub top: Vec<CommenterStats>,
    pub bottom: Vec<CommenterStats>,
}

impl Analysis {
    fn build(comments: &[Comment], commenters: &[Commenter], extremes: usize) -> Self {
        Analysis {
            total_comments: comments.len(),
            total_commenters: commenters.len(),
            ranking: rank_commenters(comments, commenters),
            top: most_active(comments, commenters, extremes),
            bottom: least_active(comments, commenters, extremes),
        }
    }
}

/// Error de comando, serializable para cruzar el límite IPC.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Ingest(#[from] ingest::IngestError),
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

// TODO(async): estos comandos son `async fn` pero `ingest::video`/`ingest::channel`
// usan `std::process::Command::output()` (bloqueante) por dentro, lo que bloquea un
// worker de Tokio y congela la UI mientras corre Node + red. Envolver la llamada en
// `tokio::task::spawn_blocking(...)` (o migrar `ingest::run` a `tokio::process::Command`
// + `.output().await`). Pendiente de verificar con build de `src-tauri`.

/// Analiza los comentarios de un video real vía el sidecar de YouTube.
#[tauri::command]
async fn analyze_video(video_id: String, api_key: String) -> Result<Analysis, CommandError> {
    let data = ingest::video(&video_id, &api_key)?;
    Ok(Analysis::build(&data.comments, &data.commenters, 5))
}

/// Analiza todos los comentarios de un canal real vía el sidecar de YouTube.
#[tauri::command]
async fn analyze_channel(channel_id: String, api_key: String) -> Result<Analysis, CommandError> {
    let data = ingest::channel(&channel_id, &api_key)?;
    Ok(Analysis::build(&data.comments, &data.commenters, 5))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            analyze_demo,
            analyze_video,
            analyze_channel
        ])
        .run(tauri::generate_context!())
        .expect("error al iniciar la aplicación Tauri");
}
