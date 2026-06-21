# Subscriptor Data Parser

App de escritorio para **conocer a tu comunidad de YouTube**: a partir de los
comentarios de un canal o un video, muestra quiénes son los que **más** comentan,
los que **menos**, y la forma completa de tu comunidad.

La idea es ayudar a que las comunidades sean más fuertes y se conozcan mejor entre
sí. Usalo de forma ética y por el bien común.

## Arquitectura

```
canal / video
   │
   ▼
[ ingest ]      sidecar Node: cliente directo de la YouTube Data API v3 (https nativo, sin deps) → JSON
   │
   ▼
[ sdp-core ]    dominio puro en Rust: modelos + rankings (testeable, sin UI)
   │
   ▼
[ src-tauri ]   app de escritorio Tauri v2: comandos #[tauri::command]
   │
   ▼
[ app ]         UI del webview (HTML/CSS/JS): espectro + padrón de comentaristas
```

| Carpeta      | Qué es                                                                 |
|--------------|------------------------------------------------------------------------|
| `crates/core`| Dominio: `Commenter`, `Comment` y los rankings. Sin red ni UI.         |
| `ingest`     | Sidecar Node: cliente directo de la [YouTube Data API v3](https://developers.google.com/youtube/v3) (`https` nativo, sin dependencias). |
| `src-tauri`  | Backend de la app de escritorio (Tauri v2).                            |
| `app`        | Frontend del webview.                                                  |
| `legacy`     | Script Node original (2021), preservado por historia.                  |

## Requisitos

- **Rust + Cargo** con el toolchain **MSVC** (`stable-x86_64-pc-windows-msvc`).
  El toolchain `windows-gnu` **no** sirve para Tauri en Windows.
- **Visual Studio C++ Build Tools** (workload "Desktop development with C++"):
  aporta el linker `link.exe` y el Windows SDK que Tauri/WebView2 necesitan.
  Sin esto, la app de escritorio no linkea.
- **WebView2 Runtime** (viene de fábrica en Windows 11).
- **Node.js** (para el sidecar de ingesta).
- **API key de YouTube Data API v3**
  ([cómo obtenerla](https://developers.google.com/youtube/v3/getting-started)).

> En Windows, una vez instaladas las Build Tools, fijá el toolchain MSVC para
> este repo: `rustup override set stable-x86_64-pc-windows-msvc`.

## Desarrollo

```bash
# Tests del dominio (Rust puro, funciona con cualquier toolchain)
cargo test -p sdp-core

# Tests del sidecar de ingesta
cd ingest && npm install && npm test

# Levantar la app de escritorio (requiere MSVC + Build Tools + @tauri-apps/cli)
cargo tauri dev
```

La app abre con un botón **"Ver con datos de ejemplo"** para explorar la interfaz
sin API key. Para datos reales, pegá el ID del canal o video y tu API key.

## Estado

Reconversión en curso del parser original (Node, 2021) a un sistema de trackeo en
Rust. Hecho: dominio con tests, sidecar de ingesta, scaffold de la app y UI.
Pendiente: persistencia local (SQLite) para histórico, y compilar/empaquetar la
app de escritorio (requiere las Build Tools de arriba).
