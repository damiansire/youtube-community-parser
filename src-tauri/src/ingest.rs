//! Puente con el sidecar de Node (`youtube-fast-api`).
//!
//! El dominio (`sdp-core`) es puro; acá vive el efecto: spawnear el proceso
//! Node, pasarle la API key por entorno y parsear su JSON a tipos del core.

use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;
use sdp_core::{Comment, Commenter};

/// Lo que emite el sidecar por stdout (ver `ingest/src/mapper.js`).
#[derive(Debug, Deserialize)]
pub struct Ingested {
    pub commenters: Vec<Commenter>,
    pub comments: Vec<Comment>,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("no se pudo ejecutar el sidecar de node: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("el sidecar falló: {0}")]
    Sidecar(String),
    #[error("respuesta del sidecar inválida: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Ruta al script del sidecar. En dev se resuelve relativo a este crate.
fn sidecar_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("ingest")
        .join("src")
        .join("index.js")
}

/// Corre el sidecar con el modo/id dados y devuelve lo ingerido.
fn run(mode_flag: &str, id: &str, api_key: &str) -> Result<Ingested, IngestError> {
    let output = Command::new("node")
        .arg(sidecar_script())
        .arg(mode_flag)
        .arg(id)
        .env("YOUTUBE_KEY_API", api_key)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(IngestError::Sidecar(stderr));
    }

    Ok(serde_json::from_slice(&output.stdout)?)
}

/// Trae los comentarios de un video.
pub fn video(video_id: &str, api_key: &str) -> Result<Ingested, IngestError> {
    run("--video", video_id, api_key)
}

/// Trae los comentarios de todos los videos de un canal.
pub fn channel(channel_id: &str, api_key: &str) -> Result<Ingested, IngestError> {
    run("--channel", channel_id, api_key)
}
