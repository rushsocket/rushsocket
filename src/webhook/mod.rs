use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::{debug, warn};

use crate::app::App;
use crate::metrics::Metrics;

/// Flush a webhook batch once it reaches this many events, even if the interval has not elapsed.
pub const WEBHOOK_BATCH_MAX: usize = 10;
pub const WEBHOOK_QUEUE_CAP: usize = 4096;
const WEBHOOK_DELIVER_PERMITS: usize = 4;

#[derive(Debug, Clone, serde::Serialize)]
pub struct WebhookEvent {
    pub name: String,
    pub channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_count: Option<usize>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct WebhookPayload {
    time_ms: u64,
    events: Vec<WebhookEvent>,
}

struct WebhookWorker {
    url: String,
    key: String,
    secret: String,
    client: reqwest::Client,
    rx: mpsc::Receiver<WebhookEvent>,
    batch_interval_ms: u64,
    semaphore: Arc<Semaphore>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    metrics: Arc<Metrics>,
}

impl WebhookWorker {
    async fn run(mut self) {
        let mut batch: Vec<WebhookEvent> = Vec::new();
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(
            self.batch_interval_ms.max(1),
        ));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;

        loop {
            tokio::select! {
                _ = self.shutdown.cancelled(), if !self.rx.is_closed() => self.rx.close(),
                event = self.rx.recv() => {
                    match event {
                        Some(e) => {
                            batch.push(e);
                            if batch.len() >= WEBHOOK_BATCH_MAX {
                                self.flush(&mut batch).await;
                            }
                        }
                        None => {
                            if !batch.is_empty() {
                                self.flush(&mut batch).await;
                            }
                            return;
                        }
                    }
                }
                _ = ticker.tick() => {
                    if !batch.is_empty() {
                        self.flush(&mut batch).await;
                    }
                }
            }
        }
    }

    async fn flush(&self, batch: &mut Vec<WebhookEvent>) {
        let Ok(permit) = self.semaphore.clone().acquire_owned().await else {
            return;
        };
        let events: Vec<WebhookEvent> = std::mem::take(batch);
        let event_count = events.len();
        let time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let payload = WebhookPayload { time_ms, events };
        let body_str = match serde_json::to_string(&payload) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "Failed to serialize webhook payload");
                return;
            }
        };
        let body = Bytes::from(body_str);
        let signature =
            crate::auth::sign(&self.secret, std::str::from_utf8(&body).unwrap_or_default());

        let client = self.client.clone();
        let url = self.url.clone();
        let key = self.key.clone();
        let metrics = self.metrics.clone();
        self.tasks.spawn(async move {
            let _permit = permit;
            for attempt in 0..3u32 {
                if attempt > 0 {
                    tokio::time::sleep(tokio::time::Duration::from_secs(1 << attempt)).await;
                }
                match client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("X-Pusher-Key", &key)
                    .header("X-Pusher-Signature", &signature)
                    .body(body.clone())
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        debug!(events = event_count, "Webhook delivered");
                        return;
                    }
                    Ok(resp) if resp.status().is_redirection() => {
                        warn!(
                            status = %resp.status(),
                            attempt = attempt + 1,
                            "Webhook redirect not followed"
                        );
                    }
                    Ok(resp) => {
                        warn!(
                            status = %resp.status(),
                            attempt = attempt + 1,
                            "Webhook delivery failed"
                        );
                    }
                    Err(e) => {
                        warn!(
                            error = %e.without_url(),
                            attempt = attempt + 1,
                            "Webhook request error"
                        );
                    }
                }
            }
            metrics
                .webhook_events_dropped
                .fetch_add(event_count as u64, std::sync::atomic::Ordering::Relaxed);
            warn!("Webhook delivery failed after 3 attempts, dropping");
        });
    }
}

/// A non-blocking handle to enqueue webhook events for an app.
#[derive(Clone)]
pub struct WebhookSender {
    tx: mpsc::Sender<WebhookEvent>,
    metrics: Arc<Metrics>,
}

impl WebhookSender {
    pub fn send(&self, event: WebhookEvent) {
        if self.tx.try_send(event).is_err() {
            self.metrics
                .webhook_events_dropped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Send a subscription_count webhook if enabled. Not sent for presence channels.
pub async fn send_subscription_count(
    state: &std::sync::Arc<crate::state::AppState>,
    app_id: &str,
    channel: &str,
    enable_subscription_count_webhook: bool,
) {
    if !enable_subscription_count_webhook {
        return;
    }
    let kind = crate::protocol::channels::ChannelKind::from_name(channel);
    if kind.is_presence() {
        return;
    }
    if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id)) {
        let count = state
            .adapter
            .get_channel_socket_count(app_id, channel)
            .await;
        wh.send(WebhookEvent::subscription_count(channel, count));
    }
}

/// Manages webhook senders for all apps.
pub struct WebhookManager {
    senders: HashMap<String, WebhookSender>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
}

impl WebhookManager {
    pub fn new(apps: &[Arc<App>], client: reqwest::Client, metrics: Arc<Metrics>) -> Self {
        let mut senders = HashMap::new();
        let shutdown = CancellationToken::new();
        let tasks = TaskTracker::new();

        for app in apps {
            if let Some(ref url) = app.webhook_url {
                let (tx, rx) = mpsc::channel(WEBHOOK_QUEUE_CAP);
                let worker = WebhookWorker {
                    url: url.clone(),
                    key: app.key.clone(),
                    secret: app.secret.clone(),
                    client: client.clone(),
                    rx,
                    batch_interval_ms: app.webhook_batch_ms,
                    semaphore: Arc::new(Semaphore::new(WEBHOOK_DELIVER_PERMITS)),
                    shutdown: shutdown.clone(),
                    tasks: tasks.clone(),
                    metrics: metrics.clone(),
                };

                tasks.spawn(worker.run());
                senders.insert(
                    app.id.clone(),
                    WebhookSender {
                        tx,
                        metrics: metrics.clone(),
                    },
                );
            }
        }

        Self {
            senders,
            shutdown,
            tasks,
        }
    }

    pub async fn drain(&self) {
        self.shutdown.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }

    pub fn get(&self, app_id: &str) -> Option<&WebhookSender> {
        self.senders.get(app_id)
    }
}

// Convenience constructors for webhook events
impl WebhookEvent {
    pub fn channel_occupied(channel: &str) -> Self {
        Self {
            name: "channel_occupied".to_string(),
            channel: channel.to_string(),
            user_id: None,
            data: None,
            event: None,
            socket_id: None,
            subscription_count: None,
        }
    }

    pub fn channel_vacated(channel: &str) -> Self {
        Self {
            name: "channel_vacated".to_string(),
            channel: channel.to_string(),
            user_id: None,
            data: None,
            event: None,
            socket_id: None,
            subscription_count: None,
        }
    }

    pub fn cache_miss(channel: &str) -> Self {
        Self {
            name: "cache_miss".to_string(),
            channel: channel.to_string(),
            user_id: None,
            data: None,
            event: None,
            socket_id: None,
            subscription_count: None,
        }
    }

    pub fn member_added(channel: &str, user_id: &str) -> Self {
        Self {
            name: "member_added".to_string(),
            channel: channel.to_string(),
            user_id: Some(user_id.to_string()),
            data: None,
            event: None,
            socket_id: None,
            subscription_count: None,
        }
    }

    pub fn member_removed(channel: &str, user_id: &str) -> Self {
        Self {
            name: "member_removed".to_string(),
            channel: channel.to_string(),
            user_id: Some(user_id.to_string()),
            data: None,
            event: None,
            socket_id: None,
            subscription_count: None,
        }
    }

    pub fn client_event(
        channel: &str,
        event_name: &str,
        data: &serde_json::Value,
        socket_id: &str,
        user_id: Option<&str>,
    ) -> Self {
        Self {
            name: "client_event".to_string(),
            channel: channel.to_string(),
            user_id: user_id.map(|s| s.to_string()),
            data: Some(data.to_string()),
            event: Some(event_name.to_string()),
            socket_id: Some(socket_id.to_string()),
            subscription_count: None,
        }
    }

    pub fn subscription_count(channel: &str, count: usize) -> Self {
        Self {
            name: "subscription_count".to_string(),
            channel: channel.to_string(),
            user_id: None,
            data: None,
            event: None,
            socket_id: None,
            subscription_count: Some(count),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_batch_max_is_ten() {
        assert_eq!(WEBHOOK_BATCH_MAX, 10);
    }

    #[tokio::test]
    async fn saturated_delivery_keeps_the_batch_with_the_worker() {
        let (_tx, rx) = mpsc::channel(1);
        let worker = WebhookWorker {
            url: String::new(),
            key: String::new(),
            secret: String::new(),
            client: reqwest::Client::new(),
            rx,
            batch_interval_ms: 1000,
            semaphore: Arc::new(Semaphore::new(0)),
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
            metrics: Arc::new(Metrics::new()),
        };
        let mut batch = vec![WebhookEvent::channel_occupied("room")];
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                worker.flush(&mut batch)
            )
            .await
            .is_err()
        );
        assert_eq!(batch.len(), 1);
    }

    #[tokio::test]
    async fn try_send_full_increments_drop_counter() {
        let metrics = Arc::new(crate::metrics::Metrics::new());
        let (tx, _rx) = mpsc::channel(WEBHOOK_QUEUE_CAP);
        let sender = WebhookSender {
            tx,
            metrics: metrics.clone(),
        };
        for _ in 0..WEBHOOK_QUEUE_CAP {
            sender.send(WebhookEvent::channel_occupied("c"));
        }
        sender.send(WebhookEvent::channel_occupied("overflow"));
        assert!(
            metrics
                .webhook_events_dropped
                .load(std::sync::atomic::Ordering::Relaxed)
                >= 1
        );
    }

    #[test]
    fn test_webhook_event_serialization() {
        let event = WebhookEvent::channel_occupied("test-channel");
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["name"], "channel_occupied");
        assert_eq!(json["channel"], "test-channel");
        assert!(json.get("user_id").is_none());
        assert!(json.get("data").is_none());
    }

    #[test]
    fn test_member_added_serialization() {
        let event = WebhookEvent::member_added("presence-chat", "user-123");
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["name"], "member_added");
        assert_eq!(json["channel"], "presence-chat");
        assert_eq!(json["user_id"], "user-123");
    }

    #[test]
    fn test_client_event_serialization() {
        let data = serde_json::json!({"msg": "hello"});
        let event = WebhookEvent::client_event(
            "private-chat",
            "client-typing",
            &data,
            "sock-1",
            Some("user-1"),
        );
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["name"], "client_event");
        assert_eq!(json["event"], "client-typing");
        assert_eq!(json["user_id"], "user-1");
        assert_eq!(json["socket_id"], "sock-1");
        assert!(json["data"].is_string());
    }

    #[test]
    fn test_cache_miss_serialization() {
        let event = WebhookEvent::cache_miss("cache-my-channel");
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["name"], "cache_miss");
        assert_eq!(json["channel"], "cache-my-channel");
        assert!(json.get("user_id").is_none());
    }

    #[test]
    fn test_subscription_count_serialization() {
        let event = WebhookEvent::subscription_count("my-channel", 42);
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["name"], "subscription_count");
        assert_eq!(json["channel"], "my-channel");
        assert_eq!(json["subscription_count"], 42);
        assert!(json.get("user_id").is_none());
    }

    #[test]
    fn test_payload_structure() {
        let payload = WebhookPayload {
            time_ms: 1700000000000,
            events: vec![
                WebhookEvent::channel_occupied("ch1"),
                WebhookEvent::channel_vacated("ch2"),
            ],
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["time_ms"], 1700000000000u64);
        assert_eq!(json["events"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_hmac_signature() {
        let secret = "test-secret";
        let body =
            r#"{"time_ms":1700000000000,"events":[{"name":"channel_occupied","channel":"ch1"}]}"#;

        let sig = crate::auth::sign(secret, body);

        // Verify it's a valid hex-encoded SHA256 (64 chars)
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn test_webhook_sender_non_blocking() {
        let metrics = Arc::new(crate::metrics::Metrics::new());
        let (tx, mut rx) = mpsc::channel(8);
        let sender = WebhookSender { tx, metrics };

        sender.send(WebhookEvent::channel_occupied("ch1"));
        sender.send(WebhookEvent::channel_vacated("ch2"));

        let e1 = rx.recv().await.unwrap();
        assert_eq!(e1.name, "channel_occupied");
        let e2 = rx.recv().await.unwrap();
        assert_eq!(e2.name, "channel_vacated");
    }

    #[tokio::test]
    async fn test_webhook_sender_drop_on_closed_channel() {
        let metrics = Arc::new(crate::metrics::Metrics::new());
        let (tx, rx) = mpsc::channel(8);
        let sender = WebhookSender {
            tx,
            metrics: metrics.clone(),
        };
        drop(rx);

        sender.send(WebhookEvent::channel_occupied("ch1"));
        assert_eq!(
            metrics
                .webhook_events_dropped
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }
}
