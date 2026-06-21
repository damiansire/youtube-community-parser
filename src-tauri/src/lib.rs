//! App de escritorio (Tauri v2). Expone el dominio `sdp-core` al webview.
//!
//! Toda la lógica vive acá (no en `main.rs`) por compatibilidad con mobile.

mod ingest;

use sdp_core::{
    least_active_of, most_active_of, rank_commenters, Comment, Commenter, CommenterStats,
};
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
        }
    }
}

/// Error de comando, serializable para cruzar el límite IPC.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Ingest(#[from] ingest::IngestError),
    #[error("la tarea de ingesta se interrumpió: {0}")]
    Join(String),
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

/// Analiza los comentarios de un video real vía el sidecar de YouTube.
///
/// `ingest::video` es bloqueante (`std::process::Command::output()` + Node +
/// red), así que se ejecuta en `spawn_blocking` para no congelar el worker de
/// Tokio que atiende este comando.
#[tauri::command]
async fn analyze_video(video_id: String, api_key: String) -> Result<Analysis, CommandError> {
    let data = tauri::async_runtime::spawn_blocking(move || ingest::video(&video_id, &api_key))
        .await
        .map_err(|e| CommandError::Join(e.to_string()))??;
    Ok(Analysis::build(&data.comments, &data.commenters, 5))
}

/// Analiza todos los comentarios de un canal real vía el sidecar de YouTube.
///
/// Igual que `analyze_video`: la ingesta es bloqueante y corre en
/// `spawn_blocking` para no bloquear el runtime.
#[tauri::command]
async fn analyze_channel(channel_id: String, api_key: String) -> Result<Analysis, CommandError> {
    let data =
        tauri::async_runtime::spawn_blocking(move || ingest::channel(&channel_id, &api_key))
            .await
            .map_err(|e| CommandError::Join(e.to_string()))??;
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
