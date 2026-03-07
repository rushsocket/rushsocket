use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use tokio::sync::watch;

use crate::adapter::Adapter;
use crate::app::AppManager;
use crate::cache::CacheDriver;
use crate::config::Config;
use crate::metrics::Metrics;
use crate::rate_limit::local::LocalRateLimiter;
use crate::webhook::WebhookManager;

pub struct AppState {
    pub config: Config,
    pub app_manager: Arc<dyn AppManager>,
    pub adapter: Arc<dyn Adapter>,
    pub metrics: Arc<Metrics>,
    pub rate_limiter: Arc<LocalRateLimiter>,
    pub cache: Arc<dyn CacheDriver>,
    pub closing: AtomicBool,
    pub started_at: Instant,
    /// Cached stats snapshot, updated by background task every 2 seconds.
    pub cached_stats: watch::Receiver<serde_json::Value>,
    pub cached_stats_tx: watch::Sender<serde_json::Value>,
    /// Cached memory usage percentage (f64 bits stored as u64), updated every 2s by stats timer.
    memory_percent_bits: AtomicU64,
    pub webhooks: Option<WebhookManager>,
}

impl AppState {
    pub fn new_with_cache(
        config: Config,
        app_manager: Arc<dyn AppManager>,
        adapter: Arc<dyn Adapter>,
        cache: Arc<dyn CacheDriver>,
    ) -> Self {
        let (cached_stats_tx, cached_stats) = watch::channel(serde_json::json!({}));

        // Initialize webhook senders for apps that have webhook_url configured
        let apps: Vec<Arc<crate::app::App>> = config.apps.iter()
            .map(|c| Arc::new(crate::app::App::from(c.clone())))
            .collect();
        let has_webhooks = apps.iter().any(|a| a.webhook_url.is_some());
        let webhooks = if has_webhooks {
            let client = reqwest::Client::new();
            Some(WebhookManager::new(&apps, client))
        } else {
            None
        };

        Self {
            config,
            app_manager,
            adapter,
            metrics: Arc::new(Metrics::new()),
            rate_limiter: Arc::new(LocalRateLimiter::new()),
            cache,
            closing: AtomicBool::new(false),
            started_at: Instant::now(),
            cached_stats,
            cached_stats_tx,
            memory_percent_bits: AtomicU64::new(0f64.to_bits()),
            webhooks,
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Relaxed)
    }

    /// Whether the server is fully ready to accept traffic (adapter initialized, not closing).
    pub fn is_ready(&self) -> bool {
        self.adapter.is_ready() && !self.is_closing()
    }

    pub fn set_closing(&self) {
        self.closing.store(true, Ordering::Relaxed);
    }

    /// Get the cached memory usage percentage (updated every 2s by stats timer).
    pub fn cached_memory_percent(&self) -> f64 {
        f64::from_bits(self.memory_percent_bits.load(Ordering::Relaxed))
    }

    /// Update the cached memory usage percentage.
    pub fn set_memory_percent(&self, pct: f64) {
        self.memory_percent_bits.store(pct.to_bits(), Ordering::Relaxed);
    }
}
