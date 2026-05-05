//! End-to-end rate-limiter test.
//!
//! Requires the model (like the rest of the `e2e_*` suite).
//! Run with: `cargo test --test e2e_rate_limit -- --ignored`

mod common;

#[tokio::test]
#[ignore]
async fn test_rate_limit_burst_then_refill() {
    let limits = nihostt::server::RuntimeLimits {
        rate_limit_per_minute: 1,
        rate_limit_burst: 1,
        shutdown_drain_secs: 2,
        ..Default::default()
    };
    let (port, shutdown) = common::start_server_with_limits(limits).await;

    let client = reqwest::Client::new();

    let (body, content_type) = {
        let boundary = "----nihostt-test-boundary";
        let wav = common::generate_wav(1, 16000);
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"test.wav\"\r\n",
        );
        body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
        body.extend_from_slice(&wav);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (body, format!("multipart/form-data; boundary={boundary}"))
    };

    let url = format!("http://127.0.0.1:{port}/v1/transcribe");

    // First request consumes the single token.
    let r1 = client
        .post(&url)
        .header("content-type", &content_type)
        .body(body.clone())
        .send()
        .await
        .expect("first request");
    assert_eq!(r1.status().as_u16(), 200, "first request should succeed");

    // Second request immediately after must be rate-limited.
    let r2 = client
        .post(&url)
        .header("content-type", &content_type)
        .body(body.clone())
        .send()
        .await
        .expect("second request");
    assert_eq!(
        r2.status().as_u16(),
        429,
        "second request should be rate-limited"
    );
    let retry_after = r2
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert_eq!(retry_after.as_deref(), Some("60"), "Retry-After must be 60");
    let body_text = r2.text().await.expect("429 body readable");
    let body: serde_json::Value = serde_json::from_str(&body_text).expect("429 body is JSON");
    assert_eq!(body["code"], "rate_limited");

    // /health should be exempt from rate limiting.
    let health = reqwest::get(format!("http://127.0.0.1:{port}/health"))
        .await
        .expect("health request");
    assert_eq!(health.status().as_u16(), 200, "/health must be exempt");

    shutdown.cancel();
}
