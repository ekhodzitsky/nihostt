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
            (440.0_f64 * 2.0 * std::f64::consts::PI * i as f64 / sample_rate as f64).sin()
                * 1000.0;
        wav.extend_from_slice(&(sample as i16).to_le_bytes());
    }
    wav
}

/// Generate raw PCM16 silence bytes.
pub fn generate_pcm16_silence(duration_s: f32, sample_rate: u32) -> Vec<u8> {
    let num_samples = (sample_rate as f32 * duration_s) as usize;
    vec![0u8; num_samples * 2]
}
