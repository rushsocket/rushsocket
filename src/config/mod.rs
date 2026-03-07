use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    pub id: String,
    pub key: String,
    pub secret: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_client_event_mode")]
    pub client_event_mode: String,
    #[serde(default = "default_true")]
    pub enable_user_authentication: bool,
    #[serde(default = "default_max_connections")]
    pub max_connections: i64,
    #[serde(default = "default_max_backend_events_per_sec")]
    pub max_backend_events_per_sec: i64,
    #[serde(default = "default_max_client_events_per_sec")]
    pub max_client_events_per_sec: i64,
    #[serde(default = "default_max_read_req_per_sec")]
    pub max_read_req_per_sec: i64,
    #[serde(default = "default_max_presence_members_per_channel")]
    pub max_presence_members_per_channel: i64,
    #[serde(default = "default_max_presence_member_size_in_kb")]
    pub max_presence_member_size_in_kb: usize,
    #[serde(default = "default_max_channel_name_length")]
    pub max_channel_name_length: usize,
    #[serde(default = "default_max_event_channels_at_once")]
    pub max_event_channels_at_once: i64,
    #[serde(default = "default_max_event_name_length")]
    pub max_event_name_length: usize,
    #[serde(default = "default_max_event_payload_in_kb")]
    pub max_event_payload_in_kb: usize,
    #[serde(default = "default_max_event_batch_size")]
    pub max_event_batch_size: i64,
    #[serde(default = "default_true")]
    pub enable_cache_channels: bool,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    #[serde(default = "default_max_message_size_in_kb")]
    pub max_message_size_in_kb: usize,
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default = "default_webhook_batch_ms")]
    pub webhook_batch_ms: u64,
    #[serde(default)]
    pub enable_subscription_count_webhook: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_metrics_port")]
    pub metrics_port: u16,
    #[serde(default = "default_true")]
    pub metrics_enabled: bool,

    #[serde(default)]
    pub ssl: SslConfig,

    #[serde(default = "default_activity_timeout")]
    pub activity_timeout: u64,
    #[serde(default = "default_server_ping_interval")]
    pub server_ping_interval: u64,
    #[serde(default = "default_server_pong_timeout")]
    pub server_pong_timeout: u64,
    #[serde(default = "default_shutdown_grace_period")]
    pub shutdown_grace_period: u64,

    #[serde(default)]
    pub apps: Vec<AppConfig>,

    #[serde(default = "default_adapter")]
    pub adapter: String,

    #[serde(default)]
    pub redis: RedisConfig,

    #[serde(default = "default_mode")]
    pub mode: String,

    #[serde(default = "default_debug")]
    pub debug: bool,

    #[serde(default = "default_memory_threshold_percent")]
    pub memory_threshold_percent: f64,

    #[serde(default = "default_cache_ttl")]
    pub cache_ttl: u64,

    /// Server-level ceiling for /events POST body size (KB).
    #[serde(default = "default_max_http_event_body_kb")]
    pub max_http_event_body_kb: usize,

    /// Server-level ceiling for /batch_events POST body size (KB).
    #[serde(default = "default_max_http_batch_body_kb")]
    pub max_http_batch_body_kb: usize,

    /// Max concurrent API requests (0 = unlimited). Health routes are never limited.
    #[serde(default = "default_max_concurrent_http_requests")]
    pub max_concurrent_http_requests: usize,

    /// Seconds to wait for WS connections to drain after SIGTERM, before stopping HTTP.
    #[serde(default = "default_shutdown_drain_period")]
    pub shutdown_drain_period: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SslConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub cert_path: Option<String>,
    #[serde(default)]
    pub key_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RedisConfig {
    #[serde(default = "default_redis_url")]
    pub url: String,
    #[serde(default = "default_redis_prefix")]
    pub prefix: String,
    #[serde(default = "default_redis_request_timeout")]
    pub request_timeout: u64,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: default_redis_url(),
            prefix: default_redis_prefix(),
            request_timeout: default_redis_request_timeout(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            metrics_port: default_metrics_port(),
            metrics_enabled: true,
            ssl: SslConfig::default(),
            activity_timeout: default_activity_timeout(),
            server_ping_interval: default_server_ping_interval(),
            server_pong_timeout: default_server_pong_timeout(),
            shutdown_grace_period: default_shutdown_grace_period(),
            apps: vec![AppConfig::default()],
            adapter: default_adapter(),
            redis: RedisConfig::default(),
            mode: default_mode(),
            debug: default_debug(),
            memory_threshold_percent: default_memory_threshold_percent(),
            cache_ttl: default_cache_ttl(),
            max_http_event_body_kb: default_max_http_event_body_kb(),
            max_http_batch_body_kb: default_max_http_batch_body_kb(),
            max_concurrent_http_requests: default_max_concurrent_http_requests(),
            shutdown_drain_period: default_shutdown_drain_period(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            id: "rushsocket-id".to_string(),
            key: "rushsocket-key".to_string(),
            secret: "rushsocket-secret".to_string(),
            enabled: true,
            client_event_mode: default_client_event_mode(),
            enable_user_authentication: true,
            max_connections: -1,                          // unlimited
            max_backend_events_per_sec: -1,               // unlimited
            max_client_events_per_sec: -1,                // unlimited
            max_read_req_per_sec: -1,                     // unlimited
            max_presence_members_per_channel: 200,
            max_presence_member_size_in_kb: 2,
            max_channel_name_length: 256,
            max_event_channels_at_once: 200,
            max_event_name_length: 256,
            max_event_payload_in_kb: 32,
            max_event_batch_size: 20,
            enable_cache_channels: true,
            allowed_origins: vec![],
            max_message_size_in_kb: 40,
            webhook_url: None,
            webhook_batch_ms: default_webhook_batch_ms(),
            enable_subscription_count_webhook: false,
        }
    }
}

fn default_client_event_mode() -> String { "all".to_string() }
fn default_max_message_size_in_kb() -> usize { 40 }
fn default_true() -> bool { true }
fn default_host() -> String { "0.0.0.0".to_string() }
fn default_port() -> u16 { 6001 }
fn default_metrics_port() -> u16 { 9100 }
fn default_activity_timeout() -> u64 { 120 }
fn default_server_ping_interval() -> u64 { 60 }
fn default_server_pong_timeout() -> u64 { 15 }
fn default_shutdown_grace_period() -> u64 { 6 }
fn default_adapter() -> String { "local".to_string() }
fn default_redis_url() -> String { "redis://127.0.0.1:6379".to_string() }
fn default_redis_prefix() -> String { "rushsocket".to_string() }
fn default_redis_request_timeout() -> u64 { 5000 }
fn default_mode() -> String { "full".to_string() }
fn default_debug() -> bool { false }
fn default_memory_threshold_percent() -> f64 { 90.0 }
fn default_cache_ttl() -> u64 { 3600 }
fn default_max_connections() -> i64 { -1 }
fn default_max_backend_events_per_sec() -> i64 { -1 }
fn default_max_client_events_per_sec() -> i64 { -1 }
fn default_max_read_req_per_sec() -> i64 { -1 }
fn default_max_presence_members_per_channel() -> i64 { 200 }
fn default_max_presence_member_size_in_kb() -> usize { 2 }
fn default_max_channel_name_length() -> usize { 256 }
fn default_max_event_channels_at_once() -> i64 { 200 }
fn default_max_event_name_length() -> usize { 256 }
fn default_max_event_payload_in_kb() -> usize { 32 }
fn default_max_event_batch_size() -> i64 { 20 }
fn default_max_http_event_body_kb() -> usize { 64 }
fn default_max_http_batch_body_kb() -> usize { 1280 }
fn default_max_concurrent_http_requests() -> usize { 256 }
fn default_shutdown_drain_period() -> u64 { 10 }
fn default_webhook_batch_ms() -> u64 { 1000 }

/// Convert an i64 limit to usize. Negative values mean unlimited (usize::MAX).
pub fn limit_to_usize(v: i64) -> usize {
    if v < 0 { usize::MAX } else { v as usize }
}

impl Config {
    pub fn load(path: &str) -> Result<Self, config::ConfigError> {
        let builder = config::Config::builder()
            .add_source(config::File::with_name(path).required(true));
        let cfg = builder.build()?;
        cfg.try_deserialize()
    }
}
