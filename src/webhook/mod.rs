use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::app::App;

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
    rx: mpsc::UnboundedReceiver<WebhookEvent>,
    batch_interval_ms: u64,
}

impl WebhookWorker {
    async fn run(mut self) {
        let mut batch: Vec<WebhookEvent> = Vec::new();
        let interval = tokio::time::Duration::from_millis(self.batch_interval_ms);

        loop {
            // Drain as many events as possible, then flush on interval or channel close
            let sleep = tokio::time::sleep(interval);
            tokio::pin!(sleep);

            loop {
                tokio::select! {
                    biased;
                    event = self.rx.recv() => {
                        match event {
                            Some(e) => batch.push(e),
                            None => {
                                // Channel closed — flush remaining and exit
                                if !batch.is_empty() {
                                    self.flush(&mut batch).await;
                                }
                                return;
                            }
                        }
                    }
                    _ = &mut sleep => {
                        break;
                    }
                }
            }

            if !batch.is_empty() {
                self.flush(&mut batch).await;
            }
        }
    }

    async fn flush(&self, batch: &mut Vec<WebhookEvent>) {
        let events: Vec<WebhookEvent> = batch.drain(..).collect();
        let time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let payload = WebhookPayload { time_ms, events };
        let body = match serde_json::to_string(&payload) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "Failed to serialize webhook payload");
                return;
            }
        };

        // HMAC-SHA256 signature
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(body.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        // Retry up to 3 times with exponential backoff
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(tokio::time::Duration::from_secs(1 << attempt)).await;
            }

            match self
                .client
                .post(&self.url)
                .header("Content-Type", "application/json")
                .header("X-Pusher-Key", &self.key)
                .header("X-Pusher-Signature", &signature)
                .body(body.clone())
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    debug!(url = %self.url, events = payload.events.len(), "Webhook delivered");
                    return;
                }
                Ok(resp) => {
                    warn!(
                        url = %self.url,
                        status = %resp.status(),
                        attempt = attempt + 1,
                        "Webhook delivery failed"
                    );
                }
                Err(e) => {
                    warn!(
                        url = %self.url,
                        error = %e,
                        attempt = attempt + 1,
                        "Webhook request error"
                    );
                }
            }
        }

        warn!(url = %self.url, "Webhook delivery failed after 3 attempts, dropping");
    }
}

/// A non-blocking handle to enqueue webhook events for an app.
#[derive(Clone)]
pub struct WebhookSender {
    tx: mpsc::UnboundedSender<WebhookEvent>,
}

impl WebhookSender {
    pub fn send(&self, event: WebhookEvent) {
        let _ = self.tx.send(event);
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
        let count = state.adapter.get_channel_socket_count(app_id, channel).await;
        wh.send(WebhookEvent::subscription_count(channel, count));
    }
}

/// Manages webhook senders for all apps.
pub struct WebhookManager {
    senders: HashMap<String, WebhookSender>,
}

impl WebhookManager {
    pub fn new(apps: &[Arc<App>], client: reqwest::Client) -> Self {
        let mut senders = HashMap::new();

        for app in apps {
            if let Some(ref url) = app.webhook_url {
                let (tx, rx) = mpsc::unbounded_channel();

                let worker = WebhookWorker {
                    url: url.clone(),
                    key: app.key.clone(),
                    secret: app.secret.clone(),
                    client: client.clone(),
                    rx,
                    batch_interval_ms: app.webhook_batch_ms,
                };

                tokio::spawn(worker.run());
                senders.insert(app.id.clone(), WebhookSender { tx });
            }
        }

        Self { senders }
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
        let event = WebhookEvent::client_event("private-chat", "client-typing", &data, "sock-1", Some("user-1"));
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
        let body = r#"{"time_ms":1700000000000,"events":[{"name":"channel_occupied","channel":"ch1"}]}"#;

        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());

        // Verify it's a valid hex-encoded SHA256 (64 chars)
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn test_webhook_sender_non_blocking() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sender = WebhookSender { tx };

        sender.send(WebhookEvent::channel_occupied("ch1"));
        sender.send(WebhookEvent::channel_vacated("ch2"));

        let e1 = rx.recv().await.unwrap();
        assert_eq!(e1.name, "channel_occupied");
        let e2 = rx.recv().await.unwrap();
        assert_eq!(e2.name, "channel_vacated");
    }

    #[tokio::test]
    async fn test_webhook_sender_drop_on_closed_channel() {
        let (tx, rx) = mpsc::unbounded_channel();
        let sender = WebhookSender { tx };
        drop(rx);

        // Should not panic — just silently drops
        sender.send(WebhookEvent::channel_occupied("ch1"));
    }
}
