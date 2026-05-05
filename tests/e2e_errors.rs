//! End-to-end error-path tests for the nihostt server.
//!
//! All tests require the ReazonSpeech model to be downloaded.
//! Run with: `cargo test --test e2e_errors -- --ignored`

mod common;

use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

fn multipart_body(
    field_name: &str,
    file_name: &str,
    mime: &str,
    data: Vec<u8>,
) -> (Vec<u8>, String) {
    let boundary = "----nihostt-test-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{field_name}\"; filename=\"{file_name}\"\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(&data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (body, format!("multipart/form-data; boundary={boundary}"))
}

// ─── 1. REST oversized body ─────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_rest_oversized_body_rejected() {
    let port = common::start_server().await;

    let client = reqwest::Client::builder()
        .build()
        .expect("Failed to build reqwest client");

    let oversized_body: Vec<u8> = vec![0u8; 51 * 1024 * 1024];
    let (body, content_type) = multipart_body("file", "test.wav", "audio/wav", oversized_body);

    let response = client
        .post(format!("http://127.0.0.1:{port}/v1/transcribe"))
        .header("content-type", content_type)
        .body(body)
        .send()
        .await
        .expect("Request should complete");

    // DefaultBodyLimit + Multipart returns 400 (multer maps body-limit error to
    // bad-request) rather than 413.
    assert_eq!(
        response.status().as_u16(),
        400,
        "Expected 400 Bad Request for oversized multipart body"
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

// ─── 3. Invalid route returns 404 ───────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_invalid_route_returns_404() {
    let port = common::start_server().await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/nonexistent"))
        .await
        .expect("GET failed");

    assert_eq!(resp.status().as_u16(), 404);
}
