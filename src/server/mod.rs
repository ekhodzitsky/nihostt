use crate::error::NihosttError;
use crate::inference::Engine;
use std::sync::Arc;

pub mod http;
pub mod rate_limit;

pub use http::{run_on_random_port, run_on_random_port_with_shutdown, run_with_config};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub origin_policy: OriginPolicy,
    pub auth: AuthConfig,
    pub limits: RuntimeLimits,
    pub metrics_enabled: bool,
    pub trust_proxy: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    pub api_keys: Vec<String>,
}

impl AuthConfig {
    pub fn is_enabled(&self) -> bool {
        !self.api_keys.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct OriginPolicy {
    pub allow_any: bool,
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeLimits {
    pub idle_timeout_secs: u64,
    pub ws_frame_max_bytes: usize,
    pub body_limit_bytes: usize,
    pub rate_limit_per_minute: u32,
    pub rate_limit_burst: u32,
    pub max_session_secs: u64,
    pub shutdown_drain_secs: u64,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 300,
            ws_frame_max_bytes: 512 * 1024,
            body_limit_bytes: 50 * 1024 * 1024,
            rate_limit_per_minute: 60,
            rate_limit_burst: 20,
            max_session_secs: 3600,
            shutdown_drain_secs: 10,
        }
    }
}
