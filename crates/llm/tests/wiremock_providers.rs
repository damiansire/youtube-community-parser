//! Tests de integración de los adaptadores de IA contra un servidor HTTP
//! mockeado (`wiremock`), sin red real. Ejercitan la **API pública** del crate
//! (lo que ve un consumidor externo): construcción del provider, `enhance` y la
//! clasificación de errores. Los detalles internos (backoff, parseo fino) se
//! cubren en los tests unitarios de `src/lib.rs`.

use sdp_core::EnhancePrompt;
use sdp_llm::{AnthropicProvider, GeminiProvider, InsightProvider, LlmError};
use secrecy::SecretString;
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
async fn anthropic_devuelve_el_texto_del_modelo() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{"type": "text", "text": "[{\"title\":\"T\"}]"}]
        })))
        .mount(&server)
        .await;

    let p = AnthropicProvider::with_base(key(), "claude-opus-4-8".into(), server.uri()).unwrap();
    assert_eq!(p.enhance(&prompt()).await.unwrap(), "[{\"title\":\"T\"}]");
}

#[tokio::test]
async fn gemini_devuelve_el_texto_del_modelo() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "candidates": [{ "content": { "parts": [{ "text": "ok" }] } }]
        })))
        .mount(&server)
        .await;

    let p = GeminiProvider::with_base(key(), "gemini-2.5-flash".into(), server.uri()).unwrap();
    assert_eq!(p.enhance(&prompt()).await.unwrap(), "ok");
}

#[tokio::test]
async fn reintenta_un_429_transitorio_y_recupera() {
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

    let p = AnthropicProvider::with_base(key(), "m".into(), server.uri()).unwrap();
    assert_eq!(p.enhance(&prompt()).await.unwrap(), "ok");
}

#[tokio::test]
async fn rate_limit_persistente_se_clasifica_como_rate_limited() {
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
async fn error_4xx_del_request_se_clasifica_como_api_sin_reintentar() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .expect(1) // un 4xx no transitorio NO se reintenta
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
async fn respuesta_sin_forma_esperada_se_clasifica_como_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{"type": "thinking", "text": "sin bloque de texto"}]
        })))
        .mount(&server)
        .await;

    let p = AnthropicProvider::with_base(key(), "m".into(), server.uri()).unwrap();
    assert!(matches!(
        p.enhance(&prompt()).await,
        Err(LlmError::Shape(_))
    ));
}
