pub mod static_manager;

use std::sync::Arc;

use crate::config::{AppConfig, limit_to_usize};

#[derive(Debug, Clone)]
pub struct App {
    pub id: String,
    pub key: String,
    pub secret: String,
    pub enabled: bool,
    pub client_event_mode: String,
    pub enable_user_authentication: bool,
    pub max_connections: i64,
    pub max_backend_events_per_sec: i64,
    pub max_client_events_per_sec: i64,
    pub max_read_req_per_sec: i64,
    pub max_presence_members_per_channel: usize,
    pub max_presence_member_size_in_kb: usize,
    pub max_channel_name_length: usize,
    pub max_event_channels_at_once: usize,
    pub max_event_name_length: usize,
    pub max_event_payload_in_kb: usize,
    pub max_event_batch_size: usize,
    pub enable_cache_channels: bool,
    pub allowed_origins: Vec<String>,
    pub max_message_size_in_kb: usize,
    pub webhook_url: Option<String>,
    pub webhook_batch_ms: u64,
    pub enable_subscription_count_webhook: bool,
}

impl From<AppConfig> for App {
    fn from(c: AppConfig) -> Self {
        Self {
            id: c.id,
            key: c.key,
            secret: c.secret,
            enabled: c.enabled,
            client_event_mode: c.client_event_mode,
            enable_user_authentication: c.enable_user_authentication,
            max_connections: c.max_connections,
            max_backend_events_per_sec: c.max_backend_events_per_sec,
            max_client_events_per_sec: c.max_client_events_per_sec,
            max_read_req_per_sec: c.max_read_req_per_sec,
            max_presence_members_per_channel: limit_to_usize(c.max_presence_members_per_channel),
            max_presence_member_size_in_kb: c.max_presence_member_size_in_kb,
            max_channel_name_length: c.max_channel_name_length,
            max_event_channels_at_once: limit_to_usize(c.max_event_channels_at_once),
            max_event_name_length: c.max_event_name_length,
            max_event_payload_in_kb: c.max_event_payload_in_kb,
            max_event_batch_size: limit_to_usize(c.max_event_batch_size),
            enable_cache_channels: c.enable_cache_channels,
            allowed_origins: c.allowed_origins,
            max_message_size_in_kb: c.max_message_size_in_kb,
            webhook_url: c.webhook_url,
            webhook_batch_ms: c.webhook_batch_ms,
            enable_subscription_count_webhook: c.enable_subscription_count_webhook,
        }
    }
}

pub trait AppManager: Send + Sync {
    fn find_by_key(&self, key: &str) -> Option<Arc<App>>;
    fn find_by_id(&self, id: &str) -> Option<Arc<App>>;
}
