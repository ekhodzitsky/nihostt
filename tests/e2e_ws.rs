//! End-to-end WebSocket protocol tests.
//!
//! All tests require the ReazonSpeech model to be downloaded.
//! Run with: `cargo test --test e2e_ws -- --ignored`

mod common;

use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------------------
// 1. Ready message validation
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // Requires model download
async fn test_ws_connect_receives_ready() {
    let port = common::start_server().await;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/v1/ws"))
        .await
        .expect("WebSocket connection failed");

    let (_, mut stream) = ws.split();

    let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("timeout waiting for Ready")
        .expect("stream ended")
        .expect("ws error");

    let text = msg.into_text().expect("expected text message");
    let ready: serde_json::Value = serde_json::from_str(&text).expect("expected JSON");

    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["language"], "ja");
}

// ---------------------------------------------------------------------------
// 2. Audio chunks → Final
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // Requires model download
async fn test_ws_audio_chunks_receives_final() {
    let port = common::start_server().await;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/v1/ws"))
        .await
        .expect("WebSocket connection failed");

    let (mut sink, mut stream) = ws.split();

    // Receive Ready
    let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("timeout waiting for Ready")
        .expect("stream ended")
        .expect("ws error");

    let text = msg.into_text().expect("expected text message");
    let ready: serde_json::Value = serde_json::from_str(&text).expect("expected JSON");
    assert_eq!(ready["type"], "ready");

    // Send 2 seconds of PCM16 silence at 16kHz
    let silence = common::generate_pcm16_silence(2.0, 16000);
    sink.send(Message::Binary(silence.into()))
        .await
        .expect("failed to send audio");

    // Send Stop
    sink.send(Message::Text(
        serde_json::to_string(&serde_json::json!({"type": "stop"}))
            .unwrap()
            .into(),
    ))
    .await
    .expect("failed to send stop");

    // Wait for Final
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("timeout waiting for message")
            .expect("stream ended")
            .expect("ws error");

        let text = msg.into_text().expect("expected text message");
        let v: serde_json::Value = serde_json::from_str(&text).expect("expected JSON");

        match v["type"].as_str().unwrap_or("") {
            "partial" => continue,
            "final" => {
                assert!(
                    v["text"].is_string(),
                    "Final message should have a text field"
                );
                break;
            }
            other => panic!("Unexpected message type: {other}, full: {text}"),
        }
    }
}
