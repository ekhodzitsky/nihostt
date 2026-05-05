//! End-to-end REST API tests for the nihostt HTTP server.
//!
//! All tests require the ReazonSpeech model to be downloaded.
//! Run with: `cargo test --test e2e_rest -- --ignored`

mod common;

use std::time::Duration;

/// Build a simple multipart/form-data body manually so we don't depend on
/// reqwest's `multipart` feature.
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

// ---------------------------------------------------------------------------
// 1. Health endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // Requires model download
async fn test_health_returns_200() {
    let port = common::start_server().await;

    let resp = tokio::time::timeout(Duration::from_secs(10), async {
        reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .expect("GET /health failed")
    })
    .await
    .expect("GET /health timed out");

    assert_eq!(resp.status(), 200);

    let text = resp.text().await.expect("Expected text body");
    let body: serde_json::Value = serde_json::from_str(&text).expect("Expected JSON body");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["model"], "reazonspeech-k2-v2");
    assert_eq!(body["language"], "ja");
}

// ---------------------------------------------------------------------------
// 2. POST /v1/transcribe — valid WAV
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // Requires model download
async fn test_transcribe_wav_returns_text() {
    let port = common::start_server().await;
    let wav = common::generate_wav(2, 16000);

    let (body, content_type) = multipart_body("file", "test.wav", "audio/wav", wav);

    let resp = tokio::time::timeout(Duration::from_secs(30), async {
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/transcribe"))
            .header("content-type", content_type)
            .body(body)
            .send()
            .await
            .expect("POST /v1/transcribe failed")
    })
    .await
    .expect("POST /v1/transcribe timed out");

    assert_eq!(resp.status(), 200);

    let text = resp.text().await.expect("Expected text body");
    let body: serde_json::Value = serde_json::from_str(&text).expect("Expected JSON body");
    assert_eq!(body["language"], "ja");
    assert!(
        body["text"].is_string(),
        "text field should be a string, got: {:?}",
        body["text"]
    );
}

// ---------------------------------------------------------------------------
// 3. POST /v1/transcribe/stream — SSE stream completes with final event
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // Requires model download
async fn test_transcribe_stream_sse_returns_events() {
    let port = common::start_server().await;
    let wav = common::generate_wav(3, 16000);

    let (body, content_type) = multipart_body("file", "test.wav", "audio/wav", wav);

    let resp = tokio::time::timeout(Duration::from_secs(60), async {
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

    assert_eq!(resp.status(), 200);

    let bytes = resp.bytes().await.expect("Expected bytes body").to_vec();
    let text = String::from_utf8_lossy(&bytes);

    let mut saw_final = false;

    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            let event: serde_json::Value =
                serde_json::from_str(data).expect("SSE data should be valid JSON");
            assert!(
                event["text"].is_string(),
                "SSE event should have text field, got: {:?}",
                event
            );
            assert!(
                event["final"].is_boolean(),
                "SSE event should have final field, got: {:?}",
                event
            );
            if event["final"].as_bool().unwrap() {
                saw_final = true;
            }
        }
    }

    assert!(saw_final, "SSE stream should contain a final event");
}
