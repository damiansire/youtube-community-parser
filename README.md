# Subscriber Data Parser

A desktop app to **get to know your YouTube community**: from the comments of a
channel or a video, it shows who comments the **most**, who comments the
**least**, and the overall shape of your community.

The goal is to help communities become stronger and get to know each other
better. Use it ethically and for the common good.

## Architecture

```
channel / video
   │
   ▼
[ src-tauri/youtube.rs ]  native client (reqwest, async): hits the
   │                       YouTube Data API v3 (commentThreads/playlistItems) → core types
   ▼
[ sdp-core ]    pure Rust domain: models + rankings (testable, UI-free)
   │
   ▼
[ src-tauri ]   Tauri v2 desktop app: #[tauri::command] commands
   │
   ▼
[ app ]         webview UI (HTML/CSS/JS): spectrum + commenter roster
```

Ingestion is **native in Rust** (there is no Node sidecar): an async `reqwest`
client that hits the [YouTube Data API v3](https://developers.google.com/youtube/v3)
directly. This makes the app **distributable** (it doesn't require `node` or the
source tree on the end user's machine).

| Folder           | What it is                                                            |
|------------------|----------------------------------------------------------------------|
| `crates/core`    | Domain: `Commenter`, `Comment` and the rankings. No network, no UI.  |
| `crates/storage` | Local SQLite persistence of the history. Each analysis is saved (idempotent upsert) and `analyze_history` re-analyzes it without spending quota again. |
| `src-tauri`      | Tauri v2 backend + native YouTube client (`youtube.rs`).             |
| `app`            | Webview frontend.                                                    |

## API key threat model

The YouTube Data API v3 key is entered in the UI and used to authenticate the
calls. Decisions and scope:

- **What it protects.** The key travels over IPC to the native process and, as
  soon as it crosses that boundary, it is wrapped in
  [`secrecy::SecretString`](https://docs.rs/secrecy): it is not logged, not
  printed via `Debug`, and is **zeroized** on drop, so it doesn't stay in the
  clear in process memory longer than necessary. The value is exposed only for
  the exact instant needed to encode it into the request URL.
- **What it does NOT protect (and why that's acceptable here).** This is a
  **single-user desktop app**: the key is **not persisted** (not saved to disk
  or to a keychain), it is requested every session. It doesn't defend against
  another process of the same user inspecting the process memory (an attacker
  with that level of access already controls the session). A full OS keychain
  (Stronghold/keyring) would be **disproportionate** for the use case and adds
  dependency surface; it stays as an optional improvement if persisting the key
  across sessions is decided later.
- **Basic hygiene.** The key goes over the HTTPS request URL (TLS encrypts it in
  transit) and never through argv or a subprocess environment (there is no
  subprocess anymore). It doesn't reintroduce the old Node-sidecar risk, where
  the key crossed into another process's `env`.

## Cost-confirmation gate threat model

Paid AI operations go through an **estimate → confirm → execute** flow bound by a
single-use token (`src-tauri/src/confirm.rs`). `estimate_*` returns the exact
cost plus a token tied to a fingerprint (operation + amount + hash of the corpus
to process); `run_*` consumes the token and re-checks the fingerprint server-side
at execution time.

- **What it protects.** **Accidental** spend (you can't execute without going
  through `estimate_*` first), **TOCTOU** (the amount or corpus can't change
  between estimating and executing without invalidating the token), and **replay**
  (the token is one-time-use). This targets the real, frequent risk: a
  double-click, an accidental retry, or a UI bug spending money on a mismatched
  amount.
- **What it does NOT protect (and why that's acceptable here).** It does **not**
  defend against a **compromised webview**. The token proves that estimate and
  execution agree — not that a human saw the modal and accepted. A front with
  arbitrary code execution (e.g. `withGlobalTauri: true` + XSS) can call
  `estimate_*` itself to mint a legitimate token and chain it into `run_*`,
  spending without ever showing a modal. Truly defending that case requires the
  **backend** to drive confirmation (a native dialog via `tauri-plugin-dialog`
  inside `run_*`), not the webview. It is deferred on purpose: for a single-user
  desktop app, a front running arbitrary code already controls the session (the
  same boundary as the API-key threat model above). It stays as an optional
  hardening if the app ever loads untrusted web content.

## Requirements

- **Rust + Cargo** with the **MSVC** toolchain (`stable-x86_64-pc-windows-msvc`).
  The `windows-gnu` toolchain does **not** work for Tauri on Windows.
- **Visual Studio C++ Build Tools** (the "Desktop development with C++"
  workload): it provides the `link.exe` linker and the Windows SDK that
  Tauri/WebView2 need. Without it, the desktop app won't link.
- **WebView2 Runtime** (ships by default on Windows 11).
- **YouTube Data API v3 key**
  ([how to get one](https://developers.google.com/youtube/v3/getting-started)).

> On Windows, once the Build Tools are installed, pin the MSVC toolchain for
> this repo: `rustup override set stable-x86_64-pc-windows-msvc`.

## Development

```bash
# Domain tests (pure Rust, works with any toolchain)
cargo test -p sdp-core

# Native YouTube client + persistence tests (requires MSVC to link)
cargo test -p sdp-desktop -p sdp-storage

# Launch the desktop app (requires MSVC + Build Tools + @tauri-apps/cli)
cargo tauri dev
```

The app opens with a **"View with sample data"** button to explore the interface
without an API key. For real data, paste the channel or video ID and your API key.

## Status

In-progress conversion of the original parser (Node, 2021) into a tracking system
in Rust. Done: tested domain, **native ingestion in Rust** (reqwest, no Node
sidecar — the app is distributable), **local persistence (SQLite) wired** to the
commands (each analysis is saved and `analyze_history` re-analyzes without
spending quota), **configurable quota caps** with partial results (F4), a **"view
accumulated history" button** that re-analyzes the local store without spending
quota, app scaffold and UI. Pending: packaging/installing the desktop app
(requires the Build Tools above).
