//! Shared test helpers for E2E tests.

#![allow(dead_code)]

use std::time::Duration;

/// Start the server on a random port and wait for it to become ready.
/// Panics if the model is not downloaded.
pub async fn start_server() -> u16 {
    let model_dir = nihostt::model::default_model_dir();
    let encoder_path = std::path::Path::new(&model_dir).join("encoder-epoch-99-avg-1.onnx");
    assert!(
        encoder_path.exists(),
        "Model not found at {}. Run `cargo run -- download` first.",
        model_dir
    );

    let engine = nihostt::inference::Engine::load_with_pool_size(&model_dir, 1)
        .expect("failed to load engine");

    let limits = nihostt::server::RuntimeLimits {
        idle_timeout_secs: 300,
        ws_frame_max_bytes: 512 * 1024,
        body_limit_bytes: 50 * 1024 * 1024,
        rate_limit_per_minute: 0,
        rate_limit_burst: 10,
        max_session_secs: 3600,
        shutdown_drain_secs: 10,
    };

    let port = nihostt::server::run_on_random_port(engine, limits)
        .await
        .expect("failed to start server");

    wait_for_ready(port, Duration::from_secs(30)).await;
    port
}

/// Start the server with custom runtime limits and a shutdown handle.
/// Returns (port, shutdown_sender).
pub async fn start_server_with_limits(
    limits: nihostt::server::RuntimeLimits,
) -> (u16, tokio_util::sync::CancellationToken) {
    let model_dir = nihostt::model::default_model_dir();
    let encoder_path = std::path::Path::new(&model_dir).join("encoder-epoch-99-avg-1.onnx");
    assert!(
        encoder_path.exists(),
        "Model not found at {}. Run `cargo run -- download` first.",
        model_dir
    );

    let engine = nihostt::inference::Engine::load_with_pool_size(&model_dir, 4)
        .expect("failed to load engine");

    let (port, shutdown) = nihostt::server::run_on_random_port_with_shutdown(engine, limits)
        .await
        .expect("failed to start server");

    wait_for_ready(port, Duration::from_secs(30)).await;
    (port, shutdown)
}

/// Connect to WebSocket, receive Ready message, return (sink, stream, ready_value).
pub async fn ws_connect(
    port: u16,
) -> (
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::Message,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    serde_json::Value,
) {
    use futures_util::StreamExt;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/v1/ws"))
        .await
        .expect("WebSocket connection failed");
    let (sink, mut stream) = ws.split();

    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout waiting for Ready")
        .expect("stream ended")
        .expect("ws error");

    let text = msg.into_text().expect("Ready should be text");
    let ready: serde_json::Value = serde_json::from_str(&text).expect("Ready should be JSON");
    assert_eq!(ready["type"], "ready", "First message should be Ready");

    (sink, stream, ready)
}

/// Parse a WebSocket message as JSON and assert its "type" field.
pub fn assert_msg_type(
    msg: tokio_tungstenite::tungstenite::Message,
    expected_type: &str,
) -> serde_json::Value {
    let text = msg.into_text().unwrap_or_else(|_| {
        panic!("Expected text message with type={expected_type}");
    });
    let v: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|_| panic!("Invalid JSON: {text}"));
    assert_eq!(
        v["type"], expected_type,
        "Expected type={expected_type}, got {:?} in {text}",
        v["type"]
    );
    v
}

/// Poll GET /health until the server responds with success or the timeout expires.
pub async fn wait_for_ready(port: u16, timeout: Duration) {
    let url = format!("http://127.0.0.1:{port}/health");
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();
    let mut delay = Duration::from_millis(50);

    loop {
        if start.elapsed() > timeout {
            panic!("Server on port {port} did not become ready within {timeout:?}");
        }
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => return,
            _ => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_millis(500));
            }
        }
    }
}

/// Generate an in-memory WAV file containing a 440Hz sine wave.
pub fn generate_wav(duration_s: u32, sample_rate: u32) -> Vec<u8> {
    let num_samples = sample_rate * duration_s;
    let data_size = num_samples * 2; // 16-bit PCM
    let file_size = 44 + data_size;

    let mut wav = Vec::with_capacity(file_size as usize);
    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(file_size - 8).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for i in 0..num_samples {
        let sample =
            (440.0_f64 * 2.0 * std::f64::consts::PI * i as f64 / sample_rate as f64).sin() * 1000.0;
        wav.extend_from_slice(&(sample as i16).to_le_bytes());
    }
    wav
}

/// Generate raw PCM16 silence bytes.
pub fn generate_pcm16_silence(duration_s: f32, sample_rate: u32) -> Vec<u8> {
    let num_samples = (sample_rate as f32 * duration_s) as usize;
    vec![0u8; num_samples * 2]
}

/// Generate raw PCM16 sine-wave bytes.
pub fn generate_pcm16_sine(duration_s: f32, sample_rate: u32, freq_hz: f32) -> Vec<u8> {
    let num_samples = (sample_rate as f32 * duration_s) as usize;
    let mut pcm = Vec::with_capacity(num_samples * 2);
    for i in 0..num_samples {
        let sample = (freq_hz as f64 * 2.0 * std::f64::consts::PI * i as f64 / sample_rate as f64)
            .sin()
            * 1000.0;
        pcm.extend_from_slice(&(sample as i16).to_le_bytes());
    }
    pcm
}

/// Read a WAV file and return its raw PCM16 data bytes.
pub fn wav_to_pcm16(path: &std::path::Path) -> Vec<u8> {
    let data = std::fs::read(path).expect("failed to read WAV file");
    let mut i = 12; // skip RIFF header + WAVE
    while i + 8 <= data.len() {
        let chunk_id = &data[i..i + 4];
        let chunk_size =
            u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
        if chunk_id == b"data" {
            assert!(
                i + 8 + chunk_size <= data.len(),
                "WAV data chunk exceeds file size"
            );
            return data[i + 8..i + 8 + chunk_size].to_vec();
        }
        i += 8 + chunk_size;
        if chunk_size % 2 == 1 {
            i += 1; // pad byte
        }
    }
    panic!("No data chunk found in WAV file");
}
