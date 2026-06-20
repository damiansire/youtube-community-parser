# Comandos de desarrollo de Subscriptor-Data-Parser.
# Un solo gate que CI y dev corren igual: `just check` antes de pushear.

# Formatear el código.
fmt:
    cargo fmt

# Gate de calidad: formato verificado + clippy sin warnings.
check:
    cargo fmt --check
    cargo clippy --workspace -- -D warnings

# Correr todos los tests del workspace.
test:
    cargo test --workspace

# Build de las crates por defecto (excluye el sidecar de Node).
build:
    cargo build

# Build optimizado.
build-release:
    cargo build --release

# Correr la app de escritorio en modo dev (requiere el front buildeado).
dev:
    cargo tauri dev
