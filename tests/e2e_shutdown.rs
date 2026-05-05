//! Graceful shutdown tests for the nihostt server.
//!
//! All tests require the ReazonSpeech model to be downloaded.
//! Run with: `cargo test --test e2e_shutdown -- --ignored --test-threads=1`

mod common;

use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------------------
// 1. Shutdown during an active WebSocket session
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_shutdown_during_ws_session() {
    let (port, shutdown) = common::start_server_with_limits(Default::default()).await;

    let (mut sink, mut stream, _ready) = common::ws_connect(port).await;

    let silence = common::generate_pcm16_silence(1.0, 48000);
    sink.send(Message::Binary(silence.into())).await.unwrap();

    shutdown.cancel();

    let result = tokio::time::timeout(Duration::from_secs(15), stream.next()).await;

    if let Err(_elapsed) = result {
        panic!("WebSocket stream did not terminate within 15s after server shutdown")
    }
}

// ---------------------------------------------------------------------------
// 2. Shutdown during an active SSE transcription stream
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_shutdown_during_sse_stream() {
    let (port, shutdown) = common::start_server_with_limits(Default::default()).await;

    let wav = common::generate_wav(60, 16000);
    let (body, content_type) = {
        let boundary = "----nihostt-test-boundary";
        let mut b = Vec::new();
        b.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        b.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"test.wav\"\r\n",
        );
        b.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
        b.extend_from_slice(&wav);
        b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (b, format!("multipart/form-data; boundary={boundary}"))
    };

    let resp = tokio::time::timeout(Duration::from_secs(30), async {
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/transcribe/stream"))
            .header("content-type", content_type)
            .body(body)
            .send()
            .await
            .expect("POST /v1/transcribe/stream failed")
    })
    .await
    .expect("POST /v1/transcribe/stream timed out");

    assert_eq!(
        resp.status(),
        200,
        "Expected 200 from /v1/transcribe/stream"
    );

    let mut bytes_stream = resp.bytes_stream();

    tokio::time::sleep(Duration::from_millis(200)).await;
    shutdown.cancel();

    let start = tokio::time::Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(10) {
            panic!("SSE bytes_stream did not terminate within 10 s after server shutdown");
        }
        match tokio::time::timeout(Duration::from_secs(1), bytes_stream.next()).await {
            Ok(None) => break,
            Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => continue,
            Err(_) => continue,
        }
    }
}
