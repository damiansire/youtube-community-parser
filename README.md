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
[ src-tauri/youtube.rs ]  cliente nativo (reqwest, async): pega directo a la
   │                       YouTube Data API v3 (commentThreads/playlistItems) → tipos del core
   ▼
[ sdp-core ]    dominio puro en Rust: modelos + rankings (testeable, sin UI)
   │
   ▼
[ src-tauri ]   app de escritorio Tauri v2: comandos #[tauri::command]
   │
   ▼
[ app ]         UI del webview (HTML/CSS/JS): espectro + padrón de comentaristas
```

La ingesta es **nativa en Rust** (no hay sidecar Node): un cliente `reqwest`
async que pega directo a la [YouTube Data API v3](https://developers.google.com/youtube/v3).
Esto hace la app **distribuible** (no requiere `node` ni el árbol de fuentes en
la máquina del usuario final).

| Carpeta      | Qué es                                                                 |
|--------------|------------------------------------------------------------------------|
| `crates/core`| Dominio: `Commenter`, `Comment` y los rankings. Sin red ni UI.         |
| `crates/storage` | Persistencia local SQLite del histórico (sin re-pegarle a la API). |
| `src-tauri`  | Backend Tauri v2 + cliente nativo de YouTube (`youtube.rs`).          |
| `app`        | Frontend del webview.                                                  |
| `legacy`     | Script Node original (2021), preservado por historia.                  |

## Modelo de amenaza de la API key

La key de la YouTube Data API v3 se ingresa en la UI y se usa para autenticar las
llamadas. Decisiones y alcance:

- **Qué protege.** La key viaja por IPC al proceso nativo y, en cuanto cruza ese
  límite, se envuelve en [`secrecy::SecretString`](https://docs.rs/secrecy):
  no se loguea, no se imprime por `Debug` y se **zeroiza** al dropearse, así que
  no queda en claro en memoria del proceso más tiempo del necesario. Se expone
  el valor solo el instante justo para encodearla en la URL del request.
- **Qué NO protege (y por qué es aceptable acá).** Es una app de **escritorio
  mono-usuario**: la key **no se persiste** (no se guarda en disco ni en
  keychain), se pide en cada sesión. No se defiende contra otro proceso del mismo
  usuario inspeccionando la memoria del proceso (un atacante con ese nivel de
  acceso ya controla la sesión). Un keychain del SO completo
  (Stronghold/keyring) sería **desproporcionado** para el caso de uso y agrega
  superficie de dependencias; queda como mejora opcional si más adelante se
  decide persistir la key entre sesiones.
- **Higiene básica.** La key va por la URL del request HTTPS (TLS la cifra en
  tránsito) y nunca por argv ni por el entorno de un subproceso (ya no hay
  subproceso). No se reintroduce el viejo riesgo del sidecar Node, donde la key
  cruzaba al `env` de otro proceso.

## Requisitos

- **Rust + Cargo** con el toolchain **MSVC** (`stable-x86_64-pc-windows-msvc`).
  El toolchain `windows-gnu` **no** sirve para Tauri en Windows.
- **Visual Studio C++ Build Tools** (workload "Desktop development with C++"):
  aporta el linker `link.exe` y el Windows SDK que Tauri/WebView2 necesitan.
  Sin esto, la app de escritorio no linkea.
- **WebView2 Runtime** (viene de fábrica en Windows 11).
- **API key de YouTube Data API v3**
  ([cómo obtenerla](https://developers.google.com/youtube/v3/getting-started)).

> En Windows, una vez instaladas las Build Tools, fijá el toolchain MSVC para
> este repo: `rustup override set stable-x86_64-pc-windows-msvc`.

## Desarrollo

```bash
# Tests del dominio (Rust puro, funciona con cualquier toolchain)
cargo test -p sdp-core

# Tests del cliente nativo de YouTube + persistencia (requiere MSVC para linkear)
cargo test -p sdp-desktop -p sdp-storage

# Levantar la app de escritorio (requiere MSVC + Build Tools + @tauri-apps/cli)
cargo tauri dev
```

La app abre con un botón **"Ver con datos de ejemplo"** para explorar la interfaz
sin API key. Para datos reales, pegá el ID del canal o video y tu API key.

## Estado

Reconversión en curso del parser original (Node, 2021) a un sistema de trackeo en
Rust. Hecho: dominio con tests, **ingesta nativa en Rust** (reqwest, sin sidecar
Node — la app es distribuible), persistencia local (SQLite) lista, scaffold de la
app y UI. Pendiente: empaquetar/instalar la app de escritorio (requiere las Build
Tools de arriba) y cablear el histórico de `sdp-storage` a los comandos.
