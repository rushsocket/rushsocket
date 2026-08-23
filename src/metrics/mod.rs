use dashmap::DashMap;
use serde_json::json;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

pub struct Metrics {
    pub connected: AtomicI64,
    pub total_connections: AtomicU64,
    pub total_disconnections: AtomicU64,
    pub ws_messages_received: AtomicU64,
    pub ws_messages_sent: AtomicU64,
    pub ws_bytes_received: AtomicU64,
    pub ws_bytes_sent: AtomicU64,
    pub http_calls: AtomicU64,
    pub http_bytes_received: AtomicU64,
    pub http_bytes_sent: AtomicU64,
    pub pubsub_broadcast_dropped: AtomicU64,
    pub webhook_events_dropped: AtomicU64,
    /// Vacate RPC waiter got fewer replies than `expected` (timeout/partial).
    pub vacate_rpc_incomplete: AtomicU64,
    pub per_app: DashMap<String, AppMetrics>,
}

pub struct AppMetrics {
    pub connected: AtomicI64,
    pub total_connections: AtomicU64,
    pub total_disconnections: AtomicU64,
    pub ws_messages_received: AtomicU64,
    pub ws_messages_sent: AtomicU64,
}

impl AppMetrics {
    pub fn new() -> Self {
        Self {
            connected: AtomicI64::new(0),
            total_connections: AtomicU64::new(0),
            total_disconnections: AtomicU64::new(0),
            ws_messages_received: AtomicU64::new(0),
            ws_messages_sent: AtomicU64::new(0),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "connected": self.connected.load(Ordering::Relaxed),
            "total_connections": self.total_connections.load(Ordering::Relaxed),
            "total_disconnections": self.total_disconnections.load(Ordering::Relaxed),
            "ws_messages_received": self.ws_messages_received.load(Ordering::Relaxed),
            "ws_messages_sent": self.ws_messages_sent.load(Ordering::Relaxed),
        })
    }
}

impl Default for AppMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            connected: AtomicI64::new(0),
            total_connections: AtomicU64::new(0),
            total_disconnections: AtomicU64::new(0),
            ws_messages_received: AtomicU64::new(0),
            ws_messages_sent: AtomicU64::new(0),
            ws_bytes_received: AtomicU64::new(0),
            ws_bytes_sent: AtomicU64::new(0),
            http_calls: AtomicU64::new(0),
            http_bytes_received: AtomicU64::new(0),
            http_bytes_sent: AtomicU64::new(0),
            pubsub_broadcast_dropped: AtomicU64::new(0),
            webhook_events_dropped: AtomicU64::new(0),
            vacate_rpc_incomplete: AtomicU64::new(0),
            per_app: DashMap::new(),
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn on_connect(&self, app_id: &str) {
        self.connected.fetch_add(1, Ordering::Relaxed);
        self.total_connections.fetch_add(1, Ordering::Relaxed);
        let app = self.per_app.entry(app_id.to_string()).or_default();
        app.connected.fetch_add(1, Ordering::Relaxed);
        app.total_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_disconnect(&self, app_id: &str) {
        self.connected.fetch_sub(1, Ordering::Relaxed);
        self.total_disconnections.fetch_add(1, Ordering::Relaxed);
        if let Some(app) = self.per_app.get(app_id) {
            app.connected.fetch_sub(1, Ordering::Relaxed);
            app.total_disconnections.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn on_ws_message_received(&self, app_id: &str, bytes: u64) {
        self.ws_messages_received.fetch_add(1, Ordering::Relaxed);
        self.ws_bytes_received.fetch_add(bytes, Ordering::Relaxed);
        if let Some(app) = self.per_app.get(app_id) {
            app.ws_messages_received.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn on_ws_message_sent(&self, app_id: &str, bytes: u64) {
        self.ws_messages_sent.fetch_add(1, Ordering::Relaxed);
        self.ws_bytes_sent.fetch_add(bytes, Ordering::Relaxed);
        if let Some(app) = self.per_app.get(app_id) {
            app.ws_messages_sent.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn on_http_call(&self, bytes_in: u64, bytes_out: u64) {
        self.http_calls.fetch_add(1, Ordering::Relaxed);
        self.http_bytes_received
            .fetch_add(bytes_in, Ordering::Relaxed);
        self.http_bytes_sent.fetch_add(bytes_out, Ordering::Relaxed);
    }
}
