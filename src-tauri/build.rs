fn main() {
    tauri_build::build();

    // tauri-build embebe el manifest de Windows (Common-Controls v6) solo en
    // los binarios de la app. Los harness de `cargo test -p sdp-desktop`
    // tambien linkean `comctl32!TaskDialogIndirect` (via tauri con el feature
    // `test`), un simbolo que solo existe en la comctl32 v6 activada por
    // manifest: sin esto el loader resuelve la v5 y el exe de tests muere al
    // cargar con STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) antes de correr
    // ningun test.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("test-manifest.xml");
        println!("cargo::rerun-if-changed=test-manifest.xml");
        println!("cargo::rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo::rustc-link-arg-tests=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
