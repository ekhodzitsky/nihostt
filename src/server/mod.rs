use crate::error::NihosttError;
use crate::inference::Engine;
use std::sync::Arc;

pub mod http;
pub mod rate_limit;

pub use http::{run_on_random_port, run_with_config};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub origin_policy: OriginPolicy,
    pub limits: RuntimeLimits,
    pub metrics_enabled: bool,
    pub trust_proxy: bool,
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
