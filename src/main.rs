use anyhow::Context;
use clap::{Parser, Subcommand};
use nihostt::server::{OriginPolicy, RuntimeLimits, ServerConfig};
use nihostt::{inference, model, server};
use std::net::IpAddr;
use std::path::Path;
use tokio::io::AsyncWriteExt;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "nihostt",
    version,
    about = "Local STT server powered by ReazonSpeech-k2-v2"
)]
struct Cli {
    /// Log level [default: info]
    #[arg(long, global = true, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start WebSocket STT server (auto-downloads model if missing)
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value_t = 9876)]
        port: u16,

        /// Bind address. Loopback by default; non-loopback requires `--bind-all`.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Model directory
        #[arg(long, default_value_t = model::default_model_dir())]
        model_dir: String,

        /// Number of concurrent inference sessions
        #[arg(long, default_value_t = 4)]
        pool_size: usize,

        /// Explicitly acknowledge binding to a non-loopback address.
        #[arg(long, default_value_t = false)]
        bind_all: bool,

        /// Additional Origin allowed to call the REST / WebSocket API (repeatable).
        #[arg(long = "allow-origin", value_name = "URL")]
        allow_origin: Vec<String>,

        /// Echo `Access-Control-Allow-Origin: *` and accept any cross-origin caller.
        #[arg(long, default_value_t = false)]
        cors_allow_any: bool,

        /// WebSocket idle timeout (seconds).
        #[arg(long, env = "NIHOSTT_IDLE_TIMEOUT_SECS", default_value_t = 300)]
        idle_timeout_secs: u64,

        /// Maximum WebSocket frame / message size (bytes).
        #[arg(long, env = "NIHOSTT_WS_FRAME_MAX_BYTES", default_value_t = 512 * 1024)]
        ws_frame_max_bytes: usize,

        /// Maximum REST request body size (bytes).
        #[arg(long, env = "NIHOSTT_BODY_LIMIT_BYTES", default_value_t = 50 * 1024 * 1024)]
        body_limit_bytes: usize,

        /// Per-IP rate limit — requests per minute (default 60). Set to 0 to disable.
        #[arg(long, env = "NIHOSTT_RATE_LIMIT_PER_MINUTE", default_value_t = 60)]
        rate_limit_per_minute: u32,

        /// Rate-limit burst size (default 10).
        #[arg(long, env = "NIHOSTT_RATE_LIMIT_BURST", default_value_t = 10)]
        rate_limit_burst: u32,

        /// Expose Prometheus metrics at `GET /metrics`.
        #[arg(long, env = "NIHOSTT_METRICS", default_value_t = false)]
        metrics: bool,

        /// Maximum wall-clock duration of a single WebSocket session (seconds).
        #[arg(long, env = "NIHOSTT_MAX_SESSION_SECS", default_value_t = 3600)]
        max_session_secs: u64,

        /// Grace window (seconds) after shutdown during which in-flight
        /// WebSocket / SSE sessions may emit their Final frames and close.
        #[arg(long, env = "NIHOSTT_SHUTDOWN_DRAIN_SECS", default_value_t = 10)]
        shutdown_drain_secs: u64,

        /// Trust `X-Forwarded-For` and `X-Real-IP` headers for rate-limit IP extraction.
        #[arg(long, env = "NIHOSTT_TRUST_PROXY", default_value_t = false)]
        trust_proxy: bool,

        /// Skip automatic INT8 quantization step after download.
        #[arg(long, env = "NIHOSTT_SKIP_QUANTIZE", default_value_t = false)]
        skip_quantize: bool,
    },

    /// Download model without starting server
    Download {
        /// Model directory
        #[arg(long, default_value_t = model::default_model_dir())]
        model_dir: String,

        /// Skip automatic INT8 quantization step after download.
        #[arg(long, env = "NIHOSTT_SKIP_QUANTIZE", default_value_t = false)]
        skip_quantize: bool,
    },

    /// Quantize encoder model to INT8 (or download pre-quantized version)
    Quantize {
        /// Model directory
        #[arg(long, default_value_t = model::default_model_dir())]
        model_dir: String,

        /// Force re-quantization even if INT8 model is already active
        #[arg(long)]
        force: bool,
    },

    /// Transcribe an audio file (offline)
    Transcribe {
        /// Path to audio file
        file: String,

        /// Model directory
        #[arg(long, default_value_t = model::default_model_dir())]
        model_dir: String,
    },
}

fn log_rss() {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status")
            && let Some(line) = status.lines().find(|l| l.starts_with("VmRSS:"))
        {
            tracing::info!("{}", line.trim());
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            && let Ok(rss) = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u64>()
        {
            tracing::info!(rss_mb = rss / 1024, "memory_after_load");
        }
    }
}

fn ensure_bind_allowed(host: &str, bind_all_flag: bool) -> anyhow::Result<()> {
    if is_loopback_host(host) {
        return Ok(());
    }
    let env_opt_in = std::env::var("NIHOSTT_ALLOW_BIND_ANY")
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    if bind_all_flag || env_opt_in {
        tracing::warn!(
            host = %host,
            "binding to non-loopback address — anyone on the network can reach this server"
        );
        return Ok(());
    }
    anyhow::bail!(
        "refusing to bind to '{host}': non-loopback addresses require \
         `--bind-all` (or env NIHOSTT_ALLOW_BIND_ANY=1) to prevent accidental \
         public exposure of local transcription"
    )
}

fn is_loopback_host(host: &str) -> bool {
    let lowered = host.trim().to_ascii_lowercase();
    if lowered == "localhost" || lowered == "::1" {
        return true;
    }
    let stripped = lowered.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = stripped.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    false
}

fn encoder_path(model_dir: &str) -> std::path::PathBuf {
    Path::new(model_dir).join("encoder-epoch-99-avg-1.onnx")
}

/// Heuristic: INT8 encoder is ~154 MB, FP32 is ~590 MB.
fn is_int8_encoder(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => meta.len() < 300 * 1024 * 1024,
        Err(_) => false,
    }
}

/// Ensure the active encoder is INT8, downloading it from HuggingFace if available
/// or falling back to native quantization of the FP32 encoder.
async fn ensure_int8_encoder(model_dir: &str, skip: bool, force: bool) -> anyhow::Result<()> {
    let fp32_path = encoder_path(model_dir);
    if skip {
        tracing::info!(
            "Skipping INT8 quantization (--skip-quantize). Engine will load the FP32 encoder."
        );
        return Ok(());
    }
    if fp32_path.exists() && is_int8_encoder(&fp32_path) && !force {
        tracing::debug!("INT8 encoder already active");
        return Ok(());
    }

    // Attempt to download pre-quantized INT8 encoder from HF.
    let int8_url = "https://huggingface.co/reazon-research/reazonspeech-k2-v2/resolve/a454b3fe1e63f4189ae3994248aeb3d31b6682f4/encoder-epoch-99-avg-1.int8.onnx";
    let client = reqwest::Client::new();
    let int8_temp = fp32_path.with_extension("int8.partial");
    tracing::info!("Attempting to download INT8 encoder from HuggingFace…");
    match client.get(int8_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let mut file = tokio::fs::File::create(&int8_temp).await?;
            let mut resp = resp;
            while let Some(chunk) = resp.chunk().await? {
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            drop(file);
            model::verify_file_sha256(&int8_temp, model::ENCODER_INT8_SHA256)
                .await
                .with_context(|| "downloaded INT8 encoder failed SHA-256 verification")?;
            if fp32_path.exists() {
                let backup = fp32_path.with_extension("fp32.onnx");
                tokio::fs::rename(&fp32_path, &backup).await?;
            }
            tokio::fs::rename(&int8_temp, &fp32_path).await?;
            tracing::info!("INT8 encoder downloaded and activated");
            return Ok(());
        }
        Ok(resp) => {
            tracing::warn!(
                "INT8 encoder not available on HF (status {}). Falling back to native quantization…",
                resp.status()
            );
        }
        Err(e) => {
            tracing::warn!(
                "Failed to reach HF for INT8 encoder ({}). Falling back to native quantization…",
                e
            );
        }
    }

    // Fallback: quantize the local FP32 encoder.
    if !fp32_path.exists() {
        anyhow::bail!("FP32 encoder not found at {}", fp32_path.display());
    }
    let int8_path = fp32_path.with_extension("int8.onnx");
    tracing::info!("Quantizing FP32 encoder to INT8 (~2 min, one-time)…");
    nihostt::quantize::quantize_model(&fp32_path, &int8_path)?;
    let backup = fp32_path.with_extension("fp32.onnx");
    tokio::fs::rename(&fp32_path, &backup).await?;
    tokio::fs::rename(&int8_path, &fp32_path).await?;
    tracing::info!("INT8 encoder saved and activated");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let directive = format!("nihostt={}", cli.log_level);
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(directive.parse()?))
        .init();

    match cli.command {
        Commands::Serve {
            port,
            host,
            model_dir,
            pool_size,
            bind_all,
            allow_origin,
            cors_allow_any,
            idle_timeout_secs,
            ws_frame_max_bytes,
            body_limit_bytes,
            rate_limit_per_minute,
            rate_limit_burst,
            metrics,
            max_session_secs,
            shutdown_drain_secs,
            trust_proxy,
            skip_quantize,
        } => {
            ensure_bind_allowed(&host, bind_all)?;
            model::ensure_model(&model_dir).await?;
            ensure_int8_encoder(&model_dir, skip_quantize, false).await?;
            let engine = inference::Engine::load_with_pool_size(&model_dir, pool_size)?;
            log_rss();
            let config = ServerConfig {
                port,
                host,
                origin_policy: OriginPolicy {
                    allow_any: cors_allow_any,
                    allowed_origins: allow_origin,
                },
                limits: RuntimeLimits {
                    idle_timeout_secs,
                    ws_frame_max_bytes,
                    body_limit_bytes,
                    rate_limit_per_minute,
                    rate_limit_burst,
                    max_session_secs,
                    shutdown_drain_secs,
                },
                metrics_enabled: metrics,
                trust_proxy,
            };
            server::run_with_config(engine, config, None).await?;
        }
        Commands::Download {
            model_dir,
            skip_quantize,
        } => {
            model::ensure_model(&model_dir).await?;
            ensure_int8_encoder(&model_dir, skip_quantize, false).await?;
            tracing::info!("Model ready at {model_dir}");
        }
        Commands::Quantize { model_dir, force } => {
            model::ensure_model(&model_dir).await?;
            ensure_int8_encoder(&model_dir, false, force).await?;
        }
        Commands::Transcribe { file, model_dir } => {
            model::ensure_model(&model_dir).await?;
            let engine = inference::Engine::load_with_pool_size(&model_dir, 1)?;
            log_rss();
            let mut guard = engine.pool.checkout().await?;
            let result = engine.transcribe_file(&file, guard.session());
            drop(guard);
            println!("{}", result?.text);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_loopback_host_recognises_common_forms() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("[::1]"));
        assert!(is_loopback_host("127.0.0.2"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.10"));
        assert!(!is_loopback_host("example.com"));
    }

    #[test]
    fn test_ensure_bind_allowed_loopback_ok() {
        ensure_bind_allowed("127.0.0.1", false).expect("loopback must be allowed");
        ensure_bind_allowed("localhost", false).expect("localhost must be allowed");
    }

    #[test]
    fn test_ensure_bind_allowed_non_loopback_requires_flag() {
        let previous = std::env::var("NIHOSTT_ALLOW_BIND_ANY").ok();
        unsafe {
            std::env::remove_var("NIHOSTT_ALLOW_BIND_ANY");
        }
        let result = ensure_bind_allowed("0.0.0.0", false);
        if let Some(v) = previous {
            unsafe {
                std::env::set_var("NIHOSTT_ALLOW_BIND_ANY", v);
            }
        }
        assert!(
            result.is_err(),
            "0.0.0.0 without --bind-all must be rejected"
        );
    }

    #[test]
    fn test_ensure_bind_allowed_explicit_flag_ok() {
        ensure_bind_allowed("0.0.0.0", true).expect("explicit --bind-all must pass");
    }
}
