//! End-to-end error-path tests for the nihostt server.
//!
//! All tests require the ReazonSpeech model to be downloaded.
//! Run with: `cargo test --test e2e_errors -- --ignored`

mod common;

use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

// ─── 1. REST oversized body ─────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_rest_oversized_body_rejected() {
    let port = common::start_server().await;

    let client = reqwest::Client::builder()
        .build()
        .expect("Failed to build reqwest client");

    let oversized_body: Vec<u8> = vec![0u8; 51 * 1024 * 1024];

    let response = client
        .post(format!("http://127.0.0.1:{port}/v1/transcribe"))
        .body(oversized_body)
        .send()
        .await
        .expect("Request should complete");

    assert_eq!(
        response.status().as_u16(),
        413,
        "Expected 413 Payload Too Large for oversized body"
    );
}

// ─── 2. WebSocket oversized frame ───────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_ws_oversized_frame_rejected() {
    let port = common::start_server().await;

    let (mut ws, _) = tokio_tungstenite::connect_async_with_config(
        format!("ws://127.0.0.1:{port}/v1/ws"),
        Some({
            let mut cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
            cfg.max_message_size = None;
            cfg.max_frame_size = None;
            cfg
        }),
        false,
    )
    .await
    .expect("WebSocket connection failed");

    let _ready = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timeout waiting for Ready")
        .expect("stream ended")
        .expect("ws error");

    let oversized: Vec<u8> = vec![0u8; 600 * 1024];
    ws.send(Message::Binary(oversized.into()))
        .await
        .expect("send should succeed on client side");

    let next = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;
    match next {
        Ok(Some(Ok(Message::Text(text)))) => {
            let v: serde_json::Value = serde_json::from_str(&text).expect("invalid JSON");
            assert_eq!(v["type"], "error");
            assert!(v["message"].as_str().unwrap().contains("too large"));
        }
        other => panic!("Expected error text message after oversized frame, got: {other:?}"),
    }

    let health = reqwest::get(format!("http://127.0.0.1:{port}/health"))
        .await
        .expect("Health check failed");
    assert!(health.status().is_success(), "Server unhealthy after test");
}

// ─── 3. REST returns 503 when pool is saturated ─────────────────────────────

#[tokio::test]
#[ignore]
async fn test_rest_saturated_pool_returns_503() {
    let limits = nihostt::server::RuntimeLimits {
        shutdown_drain_secs: 2,
        ..Default::default()
    };
    let (port, shutdown) = common::start_server_with_limits(limits).await;

    // Saturate the pool with 4 long-running REST requests.
    let long_wav = common::generate_wav(60, 16000);
    let client = reqwest::Client::new();
    let mut occupiers = Vec::new();
    for _ in 0..4 {
        let url = format!("http://127.0.0.1:{port}/v1/transcribe");
        let body = long_wav.clone();
        let c = client.clone();
        occupiers.push(tokio::spawn(async move {
            let _ = c.post(&url).body(body).send().await;
        }));
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    let response = tokio::time::timeout(
        Duration::from_secs(35),
        client
            .post(format!("http://127.0.0.1:{port}/v1/transcribe"))
            .body(common::generate_wav(1, 16000))
            .send(),
    )
    .await
    .expect("Test timed out before server returned 503")
    .expect("HTTP request failed");

    assert_eq!(
        response.status().as_u16(),
        503,
        "Expected 503 Service Unavailable when pool is saturated"
    );

    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok());
    assert_eq!(retry_after, Some("30"), "Expected Retry-After: 30");

    let body_text = response
        .text()
        .await
        .expect("Response body should be readable");
    let body: serde_json::Value =
        serde_json::from_str(&body_text).expect("Response body should be JSON");
    assert_eq!(body["code"], "timeout");

    for h in occupiers {
        let _ = h.await;
    }
    let _ = shutdown.send(());
}

// ─── 4. WebSocket returns error with retry_after_ms when pool saturated ─────

#[tokio::test]
#[ignore]
async fn test_ws_pool_saturated_returns_error_with_retry_after() {
    let limits = nihostt::server::RuntimeLimits {
        shutdown_drain_secs: 2,
        ..Default::default()
    };
    let (port, shutdown) = common::start_server_with_limits(limits).await;

    // Saturate the pool with 4 long-running REST requests.
    let long_wav = common::generate_wav(60, 16000);
    let client = reqwest::Client::new();
    let mut occupiers = Vec::new();
    for _ in 0..4 {
        let url = format!("http://127.0.0.1:{port}/v1/transcribe");
        let body = long_wav.clone();
        let c = client.clone();
        occupiers.push(tokio::spawn(async move {
            let _ = c.post(&url).body(body).send().await;
        }));
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    let (mut sink, mut stream, _ready) = common::ws_connect(port).await;

    // Send audio while pool is saturated.
    let audio = common::generate_pcm16_silence(1.0, 16000);
    sink.send(Message::Binary(audio.into()))
        .await
        .expect("send audio failed");

    // Wait for error message with retry_after_ms.
    let msg = tokio::time::timeout(Duration::from_secs(35), stream.next())
        .await
        .expect("timeout waiting for error")
        .expect("stream ended")
        .expect("ws error");

    let text = msg.into_text().expect("expected text");
    let v: serde_json::Value = serde_json::from_str(&text).expect("invalid JSON");
    assert_eq!(v["type"], "error");
    assert_eq!(v["retry_after_ms"], 30000);

    for h in occupiers {
        let _ = h.await;
    }
    let _ = shutdown.send(());
}

// ─── 5. Invalid route returns 404 ───────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_invalid_route_returns_404() {
    let port = common::start_server().await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/nonexistent"))
        .await
        .expect("GET failed");

    assert_eq!(resp.status().as_u16(), 404);
}
