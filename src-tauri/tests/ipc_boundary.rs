//! Tests desde el borde IPC (ycp-3): invocan un comando `#[tauri::command]` REAL
//! tal como lo haría el frontend (`invoke("analyze_demo")`), en vez de llamar la
//! función Rust directo. Cubren que el comando está registrado en
//! `generate_handler!` (un comando ausente ahí falla en runtime, no en
//! compile-time) y que la serialización de la respuesta cruza el límite IPC
//! intacta.
//!
//! `analyze_demo` se eligió porque corre lógica de dominio real (ranking +
//! extremos top/bottom sobre datos de muestra, vía `Analysis::build`) sin
//! necesitar `AppHandle`/`State` (no toca SQLite ni red), lo que lo hace
//! invocable con `tauri::test::mock_builder` sin tener que mockear el `Db`.
//!
//! Es un test de INTEGRACIÓN (target `[[test]]`, no `#[cfg(test)]` en el lib) a
//! propósito: `tauri::test` linkea `comctl32!TaskDialogIndirect`, que solo
//! existe en la Common-Controls v6 activada por manifest. `build.rs` embebe ese
//! manifest en los targets de test vía `rustc-link-arg-tests`, que Cargo solo
//! aplica a targets `[[test]]`; dentro del harness unitario el exe muere al
//! cargar con STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139).

use tauri::ipc::CallbackFn;
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;

fn invoke_request(cmd: &str) -> InvokeRequest {
    InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: if cfg!(any(windows, target_os = "android")) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        }
        .parse()
        .unwrap(),
        body: tauri::ipc::InvokeBody::default(),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    }
}

/// Invoca `analyze_demo` real cruzando el borde IPC (no llama la función Rust
/// directo) y verifica que la respuesta trae el análisis con datos de muestra
/// bien armados: sin solapamiento top/bottom y totales consistentes con la
/// lógica de dominio (`sample()` + `Analysis::build`, misma que cubre
/// `analysis_build_no_solapa_top_y_bottom` a nivel unitario).
#[test]
fn analyze_demo_responde_via_ipc_real() {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![sdp_desktop_lib::analyze_demo])
        .build(mock_context(noop_assets()))
        .expect("no se pudo construir la app mockeada");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("no se pudo construir el webview mockeado");

    // `Analysis` solo deriva `Serialize` (viaja del backend al frontend, nunca
    // al revés), así que deserializamos a `serde_json::Value` genérico en vez
    // de sumarle un `Deserialize` que el tipo de producción no necesita.
    let body = tauri::test::get_ipc_response(&webview, invoke_request("analyze_demo"))
        .expect("analyze_demo debe responder Ok vía IPC");
    let analysis: serde_json::Value = body
        .deserialize()
        .expect("la respuesta debe deserializar a JSON");

    assert_eq!(
        analysis["total_commenters"], 4,
        "sample() define 4 comentaristas fijos"
    );
    assert_eq!(
        analysis["total_comments"], 9,
        "sample() define 9 comentarios"
    );
    assert_eq!(
        analysis["incomplete"], false,
        "el demo no ingiere nada; nunca corta por cuota"
    );
    let ids_of = |field: &str| -> std::collections::HashSet<String> {
        analysis[field]
            .as_array()
            .expect("top/bottom deben ser arrays")
            .iter()
            .map(|s| s["channel_id"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(
        ids_of("top").is_disjoint(&ids_of("bottom")),
        "el contrato top ∩ bottom = ∅ debe sobrevivir el viaje por IPC"
    );
}

/// Un comando que existe como función Rust pero que alguien se olvidó de
/// sumar a `generate_handler!` falla SOLO en runtime (el compilador no lo
/// detecta). Este test fija esa señal: pedir un comando no registrado en
/// esta app mockeada debe volver `Err`, no colgarse ni paniquear.
#[test]
fn comando_no_registrado_falla_via_ipc_en_vez_de_colgarse() {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![sdp_desktop_lib::analyze_demo])
        .build(mock_context(noop_assets()))
        .expect("no se pudo construir la app mockeada");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("no se pudo construir el webview mockeado");

    let res = tauri::test::get_ipc_response(&webview, invoke_request("comando_inexistente"));
    assert!(
        res.is_err(),
        "invocar un comando no registrado debe fallar, no colgarse"
    );
}
