use clap::{Parser, Subcommand};
use nihostt::server::{OriginPolicy, RuntimeLimits, ServerConfig};
use nihostt::{inference, model, server};
use std::net::IpAddr;
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

        /// Per-IP rate limit — requests per minute. 0 = off (default).
        #[arg(long, env = "NIHOSTT_RATE_LIMIT_PER_MINUTE", default_value_t = 0)]
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
    },

    /// Download model without starting server
    Download {
        /// Model directory
        #[arg(long, default_value_t = model::default_model_dir())]
        model_dir: String,
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
        } => {
            ensure_bind_allowed(&host, bind_all)?;
            model::ensure_model(&model_dir).await?;
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
        Commands::Download { model_dir } => {
            model::ensure_model(&model_dir).await?;
            tracing::info!("Model ready at {model_dir}");
        }
        Commands::Transcribe { file, model_dir } => {
            model::ensure_model(&model_dir).await?;
            let engine = inference::Engine::load_with_pool_size(&model_dir, 1)?;
            log_rss();
            let mut guard = engine.pool.checkout().await?;
            let result = engine.transcribe_file(&file, &mut guard);
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
        assert!(result.is_err(), "0.0.0.0 without --bind-all must be rejected");
    }

    #[test]
    fn test_ensure_bind_allowed_explicit_flag_ok() {
        ensure_bind_allowed("0.0.0.0", true).expect("explicit --bind-all must pass");
    }
}
