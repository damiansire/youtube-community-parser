//! Capa de IA opcional multi-proveedor (F12). Define un único contrato async
//! —`InsightProvider`— y dos adaptadores intercambiables: `AnthropicProvider` y
//! `GeminiProvider`. El dominio (`sdp-core`) sigue puro: arma el prompt y parsea
//! la respuesta; este crate sólo hace la llamada HTTP.
//!
//! Sigue los patrones de `genai-app-patterns`: contrato versionable por
//! proveedor, **retry header-aware** de los fallos transitorios del server (429 y
//! 5xx/529, respetando `retry-after` con backoff exponencial) y parseo tolerante
//! (el parseo robusto del texto vive en el core).
//!
//! La key se guarda en `SecretString` (F2): no se loguea ni se imprime por
//! `Debug`, y se expone el menor tiempo posible.

use std::time::Duration;

use async_trait::async_trait;
use sdp_core::{AiProvider, EnhancePrompt};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

const ANTHROPIC_BASE: &str = "https://api.anthropic.com";
const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// Tope de tokens de salida por defecto, si el llamador no especifica uno.
/// El caller (la capa IPC) deriva un tope acorde al presupuesto del estimate
/// para que la respuesta no se trunque a mitad del JSON (ver auditoría P2).
const DEFAULT_MAX_TOKENS: u32 = 1024;
/// Reintentos ante 429 (rate limit) antes de rendirse.
const MAX_RETRIES: u32 = 2;

/// Modelos por defecto (los más capaces vigentes de cada proveedor).
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-4-8";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("error de red hablando con el proveedor de IA: {0}")]
    Http(#[from] reqwest::Error),
    #[error("el proveedor de IA respondió error HTTP {status}: {message}")]
    Api { status: u16, message: String },
    #[error("rate limit del proveedor de IA tras {0} reintentos")]
    RateLimited(u32),
    #[error("respuesta inesperada del proveedor de IA: {0}")]
    Shape(String),
}

/// Contrato del proveedor de IA. Recibe el prompt **ya construido** por el core y
/// devuelve el **texto crudo** del modelo; el parseo a tipos del dominio lo hace
/// `sdp-core` (`parse_ideas_response`). Async + dyn-compatible vía `async_trait`.
#[async_trait]
pub trait InsightProvider: Send + Sync {
    async fn enhance(&self, prompt: &EnhancePrompt) -> Result<String, LlmError>;
}

/// Backoff exponencial en segundos (1, 2, 4, …) usado cuando la respuesta no
/// trae `retry-after`.
fn backoff_secs(attempt: u32) -> u64 {
    1u64 << attempt
}

/// ¿El status HTTP es transitorio y del **lado del proveedor**? 429 (rate limit)
/// y la familia 5xx —incluido el 529 "overloaded" de Anthropic—. Son fallos que
/// el server señala explícitamente y que **no cobran**, así que reintentar es
/// seguro. Los 4xx (salvo 429) son del request: reintentarlos es inútil.
fn is_retryable_status(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

/// Envía un request reconstruyéndolo en cada intento; ante un fallo **transitorio
/// señalado por el server** (429 o 5xx/529) espera `retry-after` (o backoff) y
/// reintenta hasta `MAX_RETRIES`. Cualquier otra respuesta se devuelve tal cual.
///
/// Un **error de red** (timeout/conexión) NO se reintenta acá a propósito: sin
/// idempotency-key hacia el proveedor, un POST que falló la *respuesta* pudo
/// haberse procesado —y cobrado— igual, así que un retry ciego arriesgaría doble
/// gasto en la ruta que cuesta plata. Se propaga para que el caller decida.
async fn send_with_retry<F>(make: F) -> Result<reqwest::Response, LlmError>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt = 0;
    loop {
        let resp = make().send().await?;
        let status = resp.status().as_u16();
        if is_retryable_status(status) {
            if attempt >= MAX_RETRIES {
                // Agotados los reintentos: 429 => RateLimited; 5xx => Api con cuerpo.
                return Err(if status == 429 {
                    LlmError::RateLimited(attempt)
                } else {
                    api_error(resp).await
                });
            }
            let wait = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or_else(|| backoff_secs(attempt));
            tokio::time::sleep(Duration::from_secs(wait)).await;
            attempt += 1;
            continue;
        }
        return Ok(resp);
    }
}

/// Clasifica una respuesta no-2xx a `LlmError::Api` con un fragmento del cuerpo.
async fn api_error(resp: reqwest::Response) -> LlmError {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let message = body.chars().take(300).collect();
    LlmError::Api { status, message }
}

// ---------------------------------------------------------------------------
// Anthropic (Messages API).
// ---------------------------------------------------------------------------

pub struct AnthropicProvider {
    http: reqwest::Client,
    api_key: SecretString,
    model: String,
    base: String,
    max_tokens: u32,
}

#[derive(Deserialize)]
struct AnthropicResp {
    #[serde(default)]
    content: Vec<AnthropicBlock>,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

impl AnthropicProvider {
    pub fn new(api_key: SecretString, model: String) -> Result<Self, LlmError> {
        Self::with_base(api_key, model, ANTHROPIC_BASE.to_string())
    }

    /// Igual que `new` pero con base URL configurable (tests con wiremock).
    pub fn with_base(api_key: SecretString, model: String, base: String) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?;
        Ok(Self {
            http,
            api_key,
            model,
            base,
            max_tokens: DEFAULT_MAX_TOKENS,
        })
    }

    /// Fija el tope de tokens de salida (builder). El caller lo deriva del
    /// presupuesto del estimate para no truncar la respuesta (auditoría P2).
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    fn key(&self) -> &str {
        self.api_key.expose_secret()
    }
}

#[async_trait]
impl InsightProvider for AnthropicProvider {
    async fn enhance(&self, prompt: &EnhancePrompt) -> Result<String, LlmError> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": prompt.system,
            "messages": [{ "role": "user", "content": prompt.user }],
        });
        let url = format!("{}/v1/messages", self.base);
        let resp = send_with_retry(|| {
            self.http
                .post(&url)
                .header("x-api-key", self.key())
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
        })
        .await?;

        if !resp.status().is_success() {
            return Err(api_error(resp).await);
        }
        let parsed: AnthropicResp = resp.json().await?;
        parsed
            .content
            .into_iter()
            .find(|b| b.kind == "text")
            .and_then(|b| b.text)
            .ok_or_else(|| LlmError::Shape("respuesta de Anthropic sin bloque de texto".into()))
    }
}

// ---------------------------------------------------------------------------
// Gemini (generateContent).
// ---------------------------------------------------------------------------

pub struct GeminiProvider {
    http: reqwest::Client,
    api_key: SecretString,
    model: String,
    base: String,
    max_tokens: u32,
}

#[derive(Deserialize)]
struct GeminiResp {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
}

impl GeminiProvider {
    pub fn new(api_key: SecretString, model: String) -> Result<Self, LlmError> {
        Self::with_base(api_key, model, GEMINI_BASE.to_string())
    }

    pub fn with_base(api_key: SecretString, model: String, base: String) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?;
        Ok(Self {
            http,
            api_key,
            model,
            base,
            max_tokens: DEFAULT_MAX_TOKENS,
        })
    }

    /// Fija el tope de tokens de salida (builder). Simétrico con Anthropic: sin
    /// esto Gemini no enviaba ningún tope y el contrato de salida divergía entre
    /// proveedores (auditoría P2).
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    fn key(&self) -> &str {
        self.api_key.expose_secret()
    }
}

#[async_trait]
impl InsightProvider for GeminiProvider {
    async fn enhance(&self, prompt: &EnhancePrompt) -> Result<String, LlmError> {
        // La key va por header `x-goog-api-key` (no en la URL): evita filtrarla
        // en logs de proxy y no requiere urlencoding.
        let body = serde_json::json!({
            "systemInstruction": { "parts": [{ "text": prompt.system }] },
            "contents": [{ "role": "user", "parts": [{ "text": prompt.user }] }],
            "generationConfig": { "maxOutputTokens": self.max_tokens },
        });
        let url = format!("{}/v1beta/models/{}:generateContent", self.base, self.model);
        let resp = send_with_retry(|| {
            self.http
                .post(&url)
                .header("x-goog-api-key", self.key())
                .header("content-type", "application/json")
                .json(&body)
        })
        .await?;

        if !resp.status().is_success() {
            return Err(api_error(resp).await);
        }
        let parsed: GeminiResp = resp.json().await?;
        let text: String = parsed
            .candidates
            .into_iter()
            .next()
            .map(|c| {
                c.content
                    .parts
                    .into_iter()
                    .filter_map(|p| p.text)
                    .collect::<String>()
            })
            .unwrap_or_default();
        if text.is_empty() {
            return Err(LlmError::Shape("respuesta de Gemini sin texto".into()));
        }
        Ok(text)
    }
}

// ---------------------------------------------------------------------------
// Selección de proveedor en runtime.
// ---------------------------------------------------------------------------

/// Construye el adaptador concreto según el proveedor elegido. `model = None`
/// usa el modelo por defecto (el más capaz vigente del proveedor).
///
/// `max_tokens = None` deja el tope por defecto; el caller normalmente lo deriva
/// del presupuesto del estimate (`sdp_core::max_output_tokens_for`) para que la
/// respuesta no se trunque y se cobre por un JSON inválido (auditoría P2).
pub fn build_provider(
    provider: AiProvider,
    api_key: SecretString,
    model: Option<String>,
    max_tokens: Option<u32>,
) -> Result<Box<dyn InsightProvider>, LlmError> {
    let max_tokens = max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    match provider {
        AiProvider::Anthropic => Ok(Box::new(
            AnthropicProvider::new(
                api_key,
                model.unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string()),
            )?
            .with_max_tokens(max_tokens),
        )),
        AiProvider::Gemini => Ok(Box::new(
            GeminiProvider::new(
                api_key,
                model.unwrap_or_else(|| DEFAULT_GEMINI_MODEL.to_string()),
            )?
            .with_max_tokens(max_tokens),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn prompt() -> EnhancePrompt {
        EnhancePrompt {
            system: "sos un estratega".into(),
            user: "ideas: signals".into(),
        }
    }

    fn key() -> SecretString {
        SecretString::from("test-key".to_string())
    }

    #[tokio::test]
    async fn anthropic_extrae_el_bloque_de_texto() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [
                    {"type": "thinking", "text": "..."},
                    {"type": "text", "text": "[{\"title\":\"T\"}]"}
                ]
            })))
            .mount(&server)
            .await;

        let p =
            AnthropicProvider::with_base(key(), "claude-opus-4-8".into(), server.uri()).unwrap();
        let out = p.enhance(&prompt()).await.unwrap();
        assert_eq!(out, "[{\"title\":\"T\"}]");
    }

    #[tokio::test]
    async fn gemini_concatena_las_parts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{ "content": { "parts": [{ "text": "[{\"title\":" }, { "text": "\"T\"}]" }] } }]
            })))
            .mount(&server)
            .await;

        let p = GeminiProvider::with_base(key(), "gemini-2.5-flash".into(), server.uri()).unwrap();
        let out = p.enhance(&prompt()).await.unwrap();
        assert_eq!(out, "[{\"title\":\"T\"}]");
    }

    #[tokio::test]
    async fn reintenta_ante_429_y_luego_responde() {
        let server = MockServer::start().await;
        // Primer intento: 429 con retry-after 0 (no demora el test). Segundo: 200.
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "ok"}]
            })))
            .with_priority(2)
            .mount(&server)
            .await;

        let p =
            AnthropicProvider::with_base(key(), "claude-opus-4-8".into(), server.uri()).unwrap();
        assert_eq!(p.enhance(&prompt()).await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn reintenta_ante_5xx_transitorio_y_luego_responde() {
        let server = MockServer::start().await;
        // 503 del proveedor (transitorio, no cobra) => debe reintentar, no propagar.
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "0"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "ok"}]
            })))
            .with_priority(2)
            .mount(&server)
            .await;

        let p = AnthropicProvider::with_base(key(), "m".into(), server.uri()).unwrap();
        assert_eq!(p.enhance(&prompt()).await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn cinco_xx_persistente_se_clasifica_como_api() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(529).insert_header("retry-after", "0"))
            .mount(&server)
            .await;

        let p = AnthropicProvider::with_base(key(), "m".into(), server.uri()).unwrap();
        assert!(matches!(
            p.enhance(&prompt()).await,
            Err(LlmError::Api { status: 529, .. })
        ));
    }

    #[tokio::test]
    async fn rate_limit_persistente_falla_clasificado() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .mount(&server)
            .await;

        let p = AnthropicProvider::with_base(key(), "m".into(), server.uri()).unwrap();
        assert!(matches!(
            p.enhance(&prompt()).await,
            Err(LlmError::RateLimited(_))
        ));
    }

    #[tokio::test]
    async fn error_http_se_clasifica_como_api() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .mount(&server)
            .await;

        let p = AnthropicProvider::with_base(key(), "m".into(), server.uri()).unwrap();
        match p.enhance(&prompt()).await {
            Err(LlmError::Api { status, message }) => {
                assert_eq!(status, 400);
                assert!(message.contains("bad request"));
            }
            other => panic!("esperaba Api, fue {other:?}"),
        }
    }

    #[tokio::test]
    async fn anthropic_envia_el_max_tokens_configurado() {
        use wiremock::matchers::body_partial_json;
        let server = MockServer::start().await;
        // El mock SOLO matchea si el body trae max_tokens=2048: si el provider no
        // lo enviara (o enviara otro), no habría match y el enhance fallaría.
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(body_partial_json(serde_json::json!({ "max_tokens": 2048 })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "ok"}]
            })))
            .mount(&server)
            .await;

        let p = AnthropicProvider::with_base(key(), "m".into(), server.uri())
            .unwrap()
            .with_max_tokens(2048);
        assert_eq!(p.enhance(&prompt()).await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn gemini_envia_max_output_tokens() {
        use wiremock::matchers::body_partial_json;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
            .and(body_partial_json(
                serde_json::json!({ "generationConfig": { "maxOutputTokens": 1536 } }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{ "content": { "parts": [{ "text": "ok" }] } }]
            })))
            .mount(&server)
            .await;

        let p = GeminiProvider::with_base(key(), "gemini-2.5-flash".into(), server.uri())
            .unwrap()
            .with_max_tokens(1536);
        assert_eq!(p.enhance(&prompt()).await.unwrap(), "ok");
    }

    #[test]
    fn la_key_no_se_filtra_por_debug() {
        let k = SecretString::from("AIza-super-secreta".to_string());
        assert!(!format!("{k:?}").contains("AIza-super-secreta"));
    }

    // ── ycp-1: fallos de red/API del proveedor (timeout, respuesta malformada) ──

    #[tokio::test]
    async fn anthropic_responde_malformado_se_clasifica_como_shape() {
        let server = MockServer::start().await;
        // 200 pero sin ningun bloque "type":"text" — la forma que `enhance` no
        // sabe interpretar. No debe panicar ni confundirse con un error de red.
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "thinking", "text": "solo pensando, sin bloque de texto"}]
            })))
            .mount(&server)
            .await;

        let p = AnthropicProvider::with_base(key(), "m".into(), server.uri()).unwrap();
        let err = p.enhance(&prompt()).await.unwrap_err();
        assert!(
            matches!(err, LlmError::Shape(_)),
            "esperaba Shape, fue {err:?}"
        );
    }

    #[tokio::test]
    async fn gemini_responde_sin_candidates_se_clasifica_como_shape() {
        let server = MockServer::start().await;
        // 200 pero "candidates" vacio — respuesta bien formada como JSON pero sin
        // el contenido que `enhance` necesita.
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "candidates": [] })),
            )
            .mount(&server)
            .await;

        let p = GeminiProvider::with_base(key(), "gemini-2.5-flash".into(), server.uri()).unwrap();
        let err = p.enhance(&prompt()).await.unwrap_err();
        assert!(
            matches!(err, LlmError::Shape(_)),
            "esperaba Shape, fue {err:?}"
        );
    }

    #[tokio::test]
    async fn anthropic_ante_json_invalido_no_panica_y_devuelve_error_http() {
        let server = MockServer::start().await;
        // 200 con un body que ni siquiera es JSON valido: `resp.json()` debe
        // fallar como error de deserializacion (mapeado a Http via `#[from]`),
        // nunca panicar sobre una respuesta "exitosa" pero corrupta.
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_string("esto no es json"))
            .mount(&server)
            .await;

        let p = AnthropicProvider::with_base(key(), "m".into(), server.uri()).unwrap();
        let err = p.enhance(&prompt()).await.unwrap_err();
        assert!(
            matches!(err, LlmError::Http(_)),
            "esperaba Http, fue {err:?}"
        );
    }

    #[tokio::test]
    async fn anthropic_ante_conexion_rechazada_devuelve_error_http_no_panica() {
        // Puerto sin listener: falla a nivel de conexion TCP, no de HTTP status —
        // es el camino de "error de red" (timeout/conexion) que send_with_retry
        // deliberadamente NO reintenta (ver su doc comment) y propaga tal cual.
        let p =
            AnthropicProvider::with_base(key(), "m".into(), "http://127.0.0.1:1".into()).unwrap();
        let err = p.enhance(&prompt()).await.unwrap_err();
        assert!(
            matches!(err, LlmError::Http(_)),
            "esperaba Http, fue {err:?}"
        );
    }
}
