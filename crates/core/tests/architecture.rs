//! Test de arquitectura (auditoría P6): codifica el invariante de capas que hoy
//! solo vivía en comentarios/README. `sdp-core` es el **dominio puro** y NO debe
//! depender de las capas de efectos (`sdp-llm`, `sdp-storage`) ni de red/UI
//! (reqwest, tauri). Si un refactor invierte una dependencia, este test —y CI—
//! lo atrapan antes del merge.

use std::path::Path;

/// Lee el `Cargo.toml` de este crate y devuelve su contenido.
fn cargo_toml() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    std::fs::read_to_string(path).expect("debe poder leerse el Cargo.toml de sdp-core")
}

#[test]
fn sdp_core_no_depende_de_las_capas_de_efectos() {
    let toml = cargo_toml();
    for forbidden in ["sdp-llm", "sdp-storage", "reqwest", "tauri", "rusqlite"] {
        assert!(
            !toml.contains(forbidden),
            "sdp-core (dominio puro) no debe depender de `{forbidden}`: \
             rompe el boundary de capas. Reubicá el efecto en la capa correcta."
        );
    }
}
