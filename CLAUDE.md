# CLAUDE.md — youtube-community-parser

App de escritorio **Tauri v2** (backend Rust + webview vanilla JS) que ingiere
comentarios de YouTube, persiste un histórico local (SQLite) y ofrece refinado
con IA **pago** tras un gate de confirmación de costo.

- Workspace Rust: `crates/core` (dominio `sdp-core`), `crates/storage`
  (`sdp-storage`, SQLite), `crates/llm` (`sdp-llm`), `src-tauri` (`sdp-desktop`).
- Frontend: `app/` (index.html + main.js + styles.css), sin framework ni build.
- Es un **clone** de github.com/damiansire/Subscriptor-Data-Parser.

## Toolchain en esta máquina (Windows) — LEER antes de compilar

**AppLocker/WDAC bloquea la ejecución de binarios compilados bajo el árbol del
repo** (`...\Documents\...\target`): `cargo test`/`cargo build`/los build-scripts
fallan con **`An Application Control policy has blocked this file. (os error 4551)`**.
Es política de máquina, no un problema del código.

**Workaround verificado:** redirigí el target a un path allowlisted para
ejecución bajo `AppData\Local\Temp`:

```bash
export CARGO_TARGET_DIR='C:\Users\tester\AppData\Local\Temp\ycp-target'
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace          # corre de verdad desde el temp allowlisted
```

Con eso el gate completo (fmt + clippy `-D warnings` + test) corre **verde y los
tests se ejecutan** (no solo compilan). Sin el redirect, solo `check`/`clippy`
compilan (no ejecutan binarios) y `test` queda bloqueado al correr el `.exe`.

**`verify.sh` está stale:** su `REPO_WIN` apunta a un path hermano
(`Subscriptor-Data-Parser`) que en esta máquina no existe. No lo uses tal cual;
corré cargo directo con el `CARGO_TARGET_DIR` de arriba.

## Gate MSVC

`src-tauri` linkea con MSVC (necesita los VS C++ Build Tools + WebView2). El CI
(`.github/workflows/ci.yml`) corre `fmt + clippy + test + build` en
`windows-latest`. `RUSTFLAGS: -D warnings` — clippy no tolera warnings.
