use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use dashmap::DashMap;
use serde_json::json;

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
            per_app: DashMap::new(),
        }
    }

    pub fn on_connect(&self, app_id: &str) {
        self.connected.fetch_add(1, Ordering::Relaxed);
        self.total_connections.fetch_add(1, Ordering::Relaxed);
        let app = self.per_app
            .entry(app_id.to_string())
            .or_insert_with(AppMetrics::new);
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
        self.http_bytes_received.fetch_add(bytes_in, Ordering::Relaxed);
        self.http_bytes_sent.fetch_add(bytes_out, Ordering::Relaxed);
    }

    pub fn get_connected_for_app(&self, app_id: &str) -> i64 {
        self.per_app
            .get(app_id)
            .map(|a| a.connected.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut apps = serde_json::Map::new();
        for entry in self.per_app.iter() {
            apps.insert(entry.key().clone(), entry.value().to_json());
        }
        json!({
            "connected": self.connected.load(Ordering::Relaxed),
            "total_connections": self.total_connections.load(Ordering::Relaxed),
            "total_disconnections": self.total_disconnections.load(Ordering::Relaxed),
            "ws_messages_received": self.ws_messages_received.load(Ordering::Relaxed),
            "ws_messages_sent": self.ws_messages_sent.load(Ordering::Relaxed),
            "ws_bytes_received": self.ws_bytes_received.load(Ordering::Relaxed),
            "ws_bytes_sent": self.ws_bytes_sent.load(Ordering::Relaxed),
            "http_calls": self.http_calls.load(Ordering::Relaxed),
            "http_bytes_received": self.http_bytes_received.load(Ordering::Relaxed),
            "http_bytes_sent": self.http_bytes_sent.load(Ordering::Relaxed),
            "apps": apps,
        })
    }
}
