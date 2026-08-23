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

    /// Max |now - auth_timestamp| in seconds for REST HMAC (Pusher default 600).
    #[serde(default = "default_auth_timestamp_grace_secs")]
    pub auth_timestamp_grace_secs: u64,

    /// Bind address for the metrics listener. Defaults to loopback.
    #[serde(default = "default_metrics_host")]
    pub metrics_host: String,
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
            auth_timestamp_grace_secs: default_auth_timestamp_grace_secs(),
            metrics_host: default_metrics_host(),
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
            max_connections: -1,            // unlimited
            max_backend_events_per_sec: -1, // unlimited
            max_client_events_per_sec: -1,  // unlimited
            max_read_req_per_sec: -1,       // unlimited
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

fn default_client_event_mode() -> String {
    "all".to_string()
}
fn default_max_message_size_in_kb() -> usize {
    40
}
fn default_true() -> bool {
    true
}
fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_metrics_host() -> String {
    "127.0.0.1".to_string()
}
fn default_auth_timestamp_grace_secs() -> u64 {
    600
}
fn default_port() -> u16 {
    6001
}
fn default_metrics_port() -> u16 {
    9100
}
fn default_activity_timeout() -> u64 {
    120
}
fn default_server_ping_interval() -> u64 {
    60
}
fn default_server_pong_timeout() -> u64 {
    15
}
fn default_shutdown_grace_period() -> u64 {
    6
}
fn default_adapter() -> String {
    "local".to_string()
}
fn default_redis_url() -> String {
    "redis://127.0.0.1:6379".to_string()
}
fn default_redis_prefix() -> String {
    "rushsocket".to_string()
}
fn default_redis_request_timeout() -> u64 {
    5000
}
fn default_mode() -> String {
    "full".to_string()
}
fn default_debug() -> bool {
    false
}
fn default_memory_threshold_percent() -> f64 {
    90.0
}
fn default_cache_ttl() -> u64 {
    3600
}
fn default_max_connections() -> i64 {
    -1
}
fn default_max_backend_events_per_sec() -> i64 {
    -1
}
fn default_max_client_events_per_sec() -> i64 {
    -1
}
fn default_max_read_req_per_sec() -> i64 {
    -1
}
fn default_max_presence_members_per_channel() -> i64 {
    200
}
fn default_max_presence_member_size_in_kb() -> usize {
    2
}
fn default_max_channel_name_length() -> usize {
    256
}
fn default_max_event_channels_at_once() -> i64 {
    200
}
fn default_max_event_name_length() -> usize {
    256
}
fn default_max_event_payload_in_kb() -> usize {
    32
}
fn default_max_event_batch_size() -> i64 {
    20
}
fn default_max_http_event_body_kb() -> usize {
    64
}
fn default_max_http_batch_body_kb() -> usize {
    1280
}
fn default_max_concurrent_http_requests() -> usize {
    256
}
fn default_shutdown_drain_period() -> u64 {
    10
}
fn default_webhook_batch_ms() -> u64 {
    1000
}

/// Convert an i64 limit to usize. Negative values mean unlimited (usize::MAX).
pub fn limit_to_usize(v: i64) -> usize {
    if v < 0 { usize::MAX } else { v as usize }
}

const ROOT_KEYS: &[&str] = &[
    "host",
    "port",
    "metrics_port",
    "metrics_enabled",
    "metrics_host",
    "ssl",
    "activity_timeout",
    "server_ping_interval",
    "server_pong_timeout",
    "shutdown_grace_period",
    "apps",
    "adapter",
    "redis",
    "mode",
    "debug",
    "memory_threshold_percent",
    "cache_ttl",
    "max_http_event_body_kb",
    "max_http_batch_body_kb",
    "max_concurrent_http_requests",
    "shutdown_drain_period",
    "auth_timestamp_grace_secs",
];

const SSL_KEYS: &[&str] = &["enabled", "cert_path", "key_path"];
const REDIS_KEYS: &[&str] = &["url", "prefix", "request_timeout"];
const APP_KEYS: &[&str] = &[
    "id",
    "key",
    "secret",
    "enabled",
    "client_event_mode",
    "enable_user_authentication",
    "max_connections",
    "max_backend_events_per_sec",
    "max_client_events_per_sec",
    "max_read_req_per_sec",
    "max_presence_members_per_channel",
    "max_presence_member_size_in_kb",
    "max_channel_name_length",
    "max_event_channels_at_once",
    "max_event_name_length",
    "max_event_payload_in_kb",
    "max_event_batch_size",
    "enable_cache_channels",
    "allowed_origins",
    "max_message_size_in_kb",
    "webhook_url",
    "webhook_batch_ms",
    "enable_subscription_count_webhook",
];

fn check_known(table: &toml::Table, known: &[&str], prefix: &str) -> Result<(), String> {
    for k in table.keys() {
        if !known.contains(&k.as_str()) {
            let name = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            return Err(format!("unknown config key '{name}'"));
        }
    }
    Ok(())
}

pub(crate) fn reject_unknown_keys(root: &toml::Value) -> Result<(), String> {
    let table = root
        .as_table()
        .ok_or_else(|| "config root must be a table".to_string())?;
    check_known(table, ROOT_KEYS, "")?;
    if let Some(ssl) = table.get("ssl") {
        let t = ssl
            .as_table()
            .ok_or_else(|| "ssl must be a table".to_string())?;
        check_known(t, SSL_KEYS, "ssl")?;
    }
    if let Some(redis) = table.get("redis") {
        let t = redis
            .as_table()
            .ok_or_else(|| "redis must be a table".to_string())?;
        check_known(t, REDIS_KEYS, "redis")?;
    }
    if let Some(apps) = table.get("apps") {
        let arr = apps
            .as_array()
            .ok_or_else(|| "apps must be an array".to_string())?;
        for (i, app) in arr.iter().enumerate() {
            let t = app
                .as_table()
                .ok_or_else(|| format!("apps[{i}] must be a table"))?;
            check_known(t, APP_KEYS, &format!("apps[{i}]"))?;
        }
    }
    Ok(())
}

fn reject_unknown_file_keys(path: &str) -> Result<(), String> {
    let mut candidates = vec![std::path::PathBuf::from(path)];
    let p = std::path::Path::new(path);
    if p.extension().is_none() {
        let mut with_toml = p.to_path_buf();
        with_toml.set_extension("toml");
        candidates.push(with_toml);
    }
    for candidate in candidates {
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate).map_err(|e| e.to_string())?;
            let val: toml::Value =
                toml::from_str(&text).map_err(|e: toml::de::Error| e.to_string())?;
            return reject_unknown_keys(&val);
        }
    }
    Ok(())
}

impl Config {
    pub fn load(path: &str) -> Result<Self, config::ConfigError> {
        reject_unknown_file_keys(path).map_err(config::ConfigError::Message)?;
        let builder =
            config::Config::builder().add_source(config::File::with_name(path).required(true));
        let cfg = builder.build()?;
        let parsed: Config = cfg.try_deserialize()?;
        parsed.validate().map_err(config::ConfigError::Message)?;
        Ok(parsed)
    }

    pub fn validate(&self) -> Result<(), String> {
        match self.adapter.as_str() {
            "local" | "redis" => {}
            other => {
                return Err(format!(
                    "unknown adapter '{other}'; expected local or redis"
                ));
            }
        }
        for app in &self.apps {
            match app.client_event_mode.as_str() {
                "all" | "members" | "none" => {}
                other => {
                    return Err(format!(
                        "unknown client_event_mode '{other}' for app {}",
                        app.id
                    ));
                }
            }
        }
        let mut ids = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        for app in &self.apps {
            if !ids.insert(&app.id) {
                return Err(format!("duplicate app id '{}'", app.id));
            }
            if !keys.insert(&app.key) {
                return Err(format!("duplicate app key '{}'", app.key));
            }
        }
        if self.apps.is_empty() {
            return Err("config has no [[apps]] entries".into());
        }
        if self.ssl.enabled
            && (self
                .ssl
                .cert_path
                .as_ref()
                .map(|s| s.is_empty())
                .unwrap_or(true)
                || self
                    .ssl
                    .key_path
                    .as_ref()
                    .map(|s| s.is_empty())
                    .unwrap_or(true))
        {
            return Err("ssl.enabled requires cert_path and key_path".into());
        }
        if self.metrics_host.parse::<std::net::IpAddr>().is_err() {
            return Err(format!(
                "invalid metrics_host '{}': use an IP literal (e.g. 127.0.0.1), not a hostname",
                self.metrics_host
            ));
        }
        for app in &self.apps {
            if app.webhook_batch_ms < 1 {
                return Err(format!("webhook_batch_ms must be >= 1 for app {}", app.id));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_unknown_adapter() {
        let c = Config {
            adapter: "Redis".into(),
            ..Default::default()
        };
        assert!(c.validate().is_err());
        let c = Config {
            adapter: "local".into(),
            ..Default::default()
        };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn validate_rejects_unknown_client_event_mode() {
        let mut c = Config::default();
        c.apps[0].client_event_mode = "All".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_rejects_duplicate_app_id() {
        let mut c = Config::default();
        c.apps.push(c.apps[0].clone());
        c.apps[1].key = "other-key".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_rejects_duplicate_app_key() {
        let mut c = Config::default();
        c.apps.push(AppConfig {
            id: "other-id".into(),
            ..Default::default()
        });
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_apps() {
        let mut c = Config::default();
        c.apps.clear();
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_rejects_localhost_metrics_host() {
        let c = Config {
            metrics_host: "localhost".into(),
            ..Default::default()
        };
        let err = c.validate().unwrap_err();
        assert!(err.contains("metrics_host"), "{err}");
    }

    #[test]
    fn validate_rejects_zero_webhook_batch_ms() {
        let mut c = Config::default();
        c.apps[0].webhook_batch_ms = 0;
        let err = c.validate().unwrap_err();
        assert!(err.contains("webhook_batch_ms"), "{err}");
    }

    #[test]
    fn validate_rejects_ssl_enabled_without_paths() {
        let mut c = Config::default();
        c.ssl.enabled = true;
        assert!(c.validate().is_err());
        c.ssl.cert_path = Some("cert.pem".into());
        c.ssl.key_path = Some("key.pem".into());
        assert!(c.validate().is_ok());
    }

    #[test]
    fn reject_unknown_keys_ssl_enable_typo() {
        let val: toml::Value = toml::from_str(
            "host = \"127.0.0.1\"\n[[apps]]\nid = \"a\"\nkey = \"k\"\nsecret = \"s\"\n[ssl]\nenable = true\n",
        )
        .unwrap();
        let err = reject_unknown_keys(&val).unwrap_err();
        assert!(err.contains("ssl.enable"), "{err}");
    }

    #[test]
    fn load_rejects_ssl_enable_typo() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "rushsocket-unknown-key-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            r#"
host = "127.0.0.1"
[[apps]]
id = "a"
key = "k"
secret = "s"
[ssl]
enable = true
"#,
        )
        .unwrap();
        let err = Config::load(path.to_str().unwrap());
        let _ = std::fs::remove_file(&path);
        assert!(err.is_err(), "ssl.enable typo must fail Config::load");
    }

    #[test]
    fn default_host_is_loopback() {
        let c = Config::default();
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.metrics_host, "127.0.0.1");
        assert_eq!(c.apps[0].client_event_mode, "all");
    }
}
