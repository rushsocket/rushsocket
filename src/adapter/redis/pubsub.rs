use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use tracing::{debug, error, info, warn};

use super::request_response::{Request, RequestType, Response, ResponsePayload};
use crate::config::RedisConfig;
use crate::metrics::Metrics;

const BROADCAST_SUFFIX: &str = "#broadcast";
const REQUEST_SUFFIX: &str = "#request";
const RESPONSE_SUFFIX: &str = "#response";

/// How often each node publishes a heartbeat through Redis (seconds).
const HEARTBEAT_INTERVAL: u64 = 30;

/// If no message is received on the subscriber for this long, force reconnect (seconds).
/// This catches silently dead TCP connections (load balancer idle timeout, network partition, etc).
// Invalidate signed-in sessions before peers age this node out (two heartbeats).
const SUBSCRIBER_TIMEOUT: u64 = HEARTBEAT_INTERVAL + 15;

/// Message types that get broadcast to all nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BroadcastMessage {
    ChannelMessage {
        app_id: String,
        channel: String,
        message: String,
        excepting_id: Option<String>,
    },
    TerminateUser {
        app_id: String,
        user_id: String,
    },
    /// Periodic heartbeat to keep Redis pub/sub connections alive.
    Heartbeat,
}

/// Envelope for all pub/sub messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PubSubEnvelope {
    pub sender_node: String,
    pub payload: PubSubPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PubSubPayload {
    Broadcast(BroadcastMessage),
    Request(Request),
    Response(Response),
}

/// Remove the reply channel even if the HTTP request is cancelled.
struct PendingRequest<'a> {
    id: String,
    pending: &'a DashMap<String, mpsc::Sender<Response>>,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        self.pending.remove(&self.id);
    }
}

pub struct RpcBatch {
    pub expected: usize,
    pub complete: bool,
    pub responses: Vec<ResponsePayload>,
}

pub struct RedisPubSub {
    /// Redis URL for creating connections.
    redis_url: String,
    /// Connection for broadcast publishes (channel messages, heartbeats).
    broadcast_conn: redis::aio::ConnectionManager,
    /// Connection for control-plane publishes (requests, responses).
    /// Separated from broadcast to prevent head-of-line blocking under bursty fan-out.
    control_conn: redis::aio::ConnectionManager,
    /// Channel prefix.
    prefix: String,
    /// Timeout for cross-node requests.
    request_timeout: Duration,
    /// Pending request/response tracking.
    pending: DashMap<String, mpsc::Sender<Response>>,
    /// Node ID of this instance.
    node_id: String,
    /// Known peer nodes: node_id -> last-seen instant (replaces PUBSUB NUMSUB).
    known_nodes: DashMap<String, std::time::Instant>,
    /// Whether subscriptions are live and peer discovery is complete.
    subscribed: AtomicBool,
    pub(crate) metrics: Arc<Metrics>,
}

struct BroadcastWork {
    sender_node: String,
    payload: BroadcastMessage,
}

struct ResponseWork {
    target_node: String,
    response: Response,
}

impl RedisPubSub {
    pub async fn new(
        config: &RedisConfig,
        node_id: &str,
        metrics: Arc<Metrics>,
    ) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(config.url.as_str())?;
        let broadcast_conn = client.get_connection_manager().await?;
        let control_conn = client.get_connection_manager().await?;

        Ok(Self {
            redis_url: config.url.clone(),
            broadcast_conn,
            control_conn,
            prefix: config.prefix.clone(),
            request_timeout: Duration::from_millis(config.request_timeout),
            pending: DashMap::new(),
            node_id: node_id.to_string(),
            known_nodes: DashMap::new(),
            subscribed: AtomicBool::new(false),
            metrics,
        })
    }

    fn broadcast_channel(&self) -> String {
        format!("{}{}", self.prefix, BROADCAST_SUFFIX)
    }

    fn request_channel(&self) -> String {
        format!("{}{}", self.prefix, REQUEST_SUFFIX)
    }

    fn response_channel(&self) -> String {
        format!("{}{}:{}", self.prefix, RESPONSE_SUFFIX, self.node_id)
    }

    /// Publish a broadcast message to all nodes.
    pub async fn publish(
        &self,
        sender_node: &str,
        message: &BroadcastMessage,
    ) -> Result<(), redis::RedisError> {
        let envelope = PubSubEnvelope {
            sender_node: sender_node.to_string(),
            payload: PubSubPayload::Broadcast(message.clone()),
        };
        let json = serde_json::to_string(&envelope).unwrap_or_default();
        self.broadcast_conn
            .clone()
            .publish(self.broadcast_channel(), json)
            .await
    }

    /// Count subscribers at publication and require a distinct response from
    /// each known peer as well. A missing subscriber or lost response fails closed.
    pub async fn request(
        &self,
        sender_node: &str,
        request_type: RequestType,
    ) -> Result<RpcBatch, redis::RedisError> {
        let mut required: HashSet<_> = self.get_known_node_ids().into_iter().collect();
        if matches!(request_type, RequestType::DiscoverPeers) {
            required.clear();
        }
        required.insert(self.node_id.clone());
        let pending = PendingRequest {
            id: uuid::Uuid::new_v4().to_string(),
            pending: &self.pending,
        };
        let (tx, mut rx) = mpsc::channel(required.len().max(64));
        self.pending.insert(pending.id.clone(), tx);
        let envelope = PubSubEnvelope {
            sender_node: sender_node.to_string(),
            payload: PubSubPayload::Request(Request {
                request_id: pending.id.clone(),
                sender_node: sender_node.to_string(),
                request_type,
            }),
        };
        let json = serde_json::to_string(&envelope).expect("serializable RPC request");
        let subscribers: usize = self
            .control_conn
            .clone()
            .publish(self.request_channel(), json)
            .await?;
        let expected = subscribers.max(required.len());
        let mut responses = HashMap::new();
        let complete = tokio::time::timeout(self.request_timeout, async {
            while responses.len() < expected || !required.is_empty() {
                let Some(response) = rx.recv().await else {
                    return false;
                };
                required.remove(&response.responder_node);
                responses.insert(response.responder_node, response.payload);
            }
            true
        })
        .await
        .unwrap_or(false);
        responses.remove(&self.node_id);
        Ok(RpcBatch {
            expected: expected.saturating_sub(1),
            complete,
            responses: responses.into_values().collect(),
        })
    }

    async fn send_response(
        &self,
        target_node: &str,
        response: Response,
    ) -> Result<(), redis::RedisError> {
        let channel = format!("{}{}:{}", self.prefix, RESPONSE_SUFFIX, target_node);
        let envelope = PubSubEnvelope {
            sender_node: self.node_id.clone(),
            payload: PubSubPayload::Response(response),
        };
        let json = serde_json::to_string(&envelope).expect("serializable RPC response");
        self.control_conn.clone().publish(channel, json).await
    }

    /// Record that a peer node is alive (called on any message from that node).
    fn record_peer(&self, node_id: &str) {
        self.known_nodes
            .insert(node_id.to_string(), std::time::Instant::now());
    }

    /// Whether this node's Redis subscriptions are live and peer discovery is complete.
    pub fn is_subscribed(&self) -> bool {
        self.subscribed.load(Ordering::Acquire)
    }

    /// Get IDs of all known peer nodes (excluding self).
    pub fn get_known_node_ids(&self) -> Vec<String> {
        let stale_threshold = Duration::from_secs(HEARTBEAT_INTERVAL * 2);
        let now = std::time::Instant::now();
        self.known_nodes
            .retain(|_, last_seen| now.duration_since(*last_seen) < stale_threshold);
        self.known_nodes.iter().map(|e| e.key().clone()).collect()
    }

    /// Subscribe and listen for messages. Returns when worker senders are Closed.
    /// Subscribe-socket death reconnects inside this call with the same senders.
    pub async fn listen(self: Arc<Self>, adapter: Arc<super::RedisAdapter>) {
        let (fanout_tx, mut fanout_rx) = mpsc::channel::<BroadcastWork>(1024);
        let (resp_tx, mut resp_rx) = mpsc::channel::<ResponseWork>(1024);

        let adapter_fan = adapter.clone();
        tokio::spawn(async move {
            while let Some(work) = fanout_rx.recv().await {
                adapter_fan
                    .handle_broadcast(&work.sender_node, &work.payload)
                    .await;
            }
        });
        let responder = self.clone();
        tokio::spawn(async move {
            while let Some(work) = resp_rx.recv().await {
                if let Err(e) = responder
                    .send_response(&work.target_node, work.response)
                    .await
                {
                    warn!("Failed to send response: {}", e);
                }
            }
        });

        let broadcast_ch = self.broadcast_channel();
        let request_ch = self.request_channel();
        let response_ch = self.response_channel();

        loop {
            let result = self
                .subscribe_loop(
                    &adapter,
                    &broadcast_ch,
                    &request_ch,
                    &response_ch,
                    &fanout_tx,
                    &resp_tx,
                )
                .await;
            if self.subscribed.swap(false, Ordering::AcqRel) {
                adapter.local.close_signed_in_sockets();
            }
            match result {
                Ok(()) => {
                    info!("Redis listen ended (worker senders closed)");
                    break;
                }
                Err(e) => {
                    error!("Redis subscribe loop error: {}, reconnecting in 1s...", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Periodically publish a heartbeat to keep the Redis pub/sub connection alive.
    /// This ensures traffic flows even when no real events are happening,
    /// preventing silent TCP death from load balancer idle timeouts.
    pub(crate) async fn heartbeat_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(HEARTBEAT_INTERVAL)).await;

            if let Err(e) = self
                .publish(&self.node_id, &BroadcastMessage::Heartbeat)
                .await
            {
                warn!("Failed to send heartbeat: {}", e);
            } else {
                debug!("Heartbeat sent");
            }
        }
    }

    async fn subscribe_loop(
        self: &Arc<Self>,
        adapter: &Arc<super::RedisAdapter>,
        broadcast_ch: &str,
        request_ch: &str,
        response_ch: &str,
        fanout_tx: &mpsc::Sender<BroadcastWork>,
        resp_tx: &mpsc::Sender<ResponseWork>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let client = redis::Client::open(self.redis_url.as_str())?;
        let mut pubsub = client.get_async_pubsub().await?;
        pubsub.subscribe(broadcast_ch).await?;
        pubsub.subscribe(request_ch).await?;
        pubsub.subscribe(response_ch).await?;

        info!(
            "Redis pub/sub subscribed to: {}, {}, {}",
            broadcast_ch, request_ch, response_ch
        );

        // Announce ourselves now that subscriptions are live.
        // This ensures peers discover us and we can receive their requests.
        // Runs on every (re)connect so peers re-learn us after a reconnect too.
        if let Err(e) = self
            .publish(&self.node_id, &BroadcastMessage::Heartbeat)
            .await
        {
            warn!("Failed to send announce heartbeat: {}", e);
        }

        // Poll discovery alongside the subscription stream so its own reply can
        // arrive. Dropping this stream also cancels its readiness transition.
        let discovery = async {
            let batch = self
                .request(&self.node_id, RequestType::DiscoverPeers)
                .await?;
            if !batch.complete {
                return Err("peer discovery incomplete".into());
            }
            adapter.refresh_heartbeat().await?;
            self.subscribed.store(true, Ordering::Release);
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        };
        tokio::pin!(discovery);
        let mut discovering = true;

        let mut msg_stream = pubsub.on_message();

        use futures_util::StreamExt;

        loop {
            // Wait for next message OR timeout if nothing received for too long.
            // This catches silently dead connections that never error — they just
            // stop delivering messages.
            let msg = tokio::select! {
                result = &mut discovery, if discovering => {
                    result?;
                    discovering = false;
                    continue;
                }
                msg = tokio::time::timeout(Duration::from_secs(SUBSCRIBER_TIMEOUT), msg_stream.next()) => msg,
            };

            match msg {
                Ok(Some(msg)) => {
                    let payload: String = match msg.get_payload() {
                        Ok(p) => p,
                        Err(e) => {
                            warn!("Failed to get message payload: {}", e);
                            continue;
                        }
                    };
                    if payload.is_empty() {
                        continue;
                    }

                    let envelope: PubSubEnvelope = match serde_json::from_str(&payload) {
                        Ok(e) => e,
                        Err(e) => {
                            warn!("Failed to deserialize pub/sub message: {}", e);
                            continue;
                        }
                    };

                    if envelope.sender_node != self.node_id {
                        self.record_peer(&envelope.sender_node);
                    } else if matches!(envelope.payload, PubSubPayload::Broadcast(_)) {
                        continue;
                    }

                    match envelope.payload {
                        PubSubPayload::Broadcast(BroadcastMessage::Heartbeat) => {
                            debug!("Heartbeat received from {}", envelope.sender_node);
                        }
                        PubSubPayload::Broadcast(broadcast) => {
                            match fanout_tx.try_send(BroadcastWork {
                                sender_node: envelope.sender_node,
                                payload: broadcast,
                            }) {
                                Ok(()) => {}
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    self.metrics
                                        .pubsub_broadcast_dropped
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                            }
                        }
                        PubSubPayload::Request(request) => {
                            let response_payload = adapter.handle_request(&request).await;
                            let response = Response {
                                request_id: request.request_id,
                                responder_node: self.node_id.clone(),
                                payload: response_payload,
                            };
                            match resp_tx.try_send(ResponseWork {
                                target_node: request.sender_node,
                                response,
                            }) {
                                Ok(()) => {}
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    warn!("Redis RPC response queue full")
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                            }
                        }
                        PubSubPayload::Response(response) => {
                            if let Some(tx) = self.pending.get(&response.request_id) {
                                let _ = tx.try_send(response);
                            }
                        }
                    }
                }
                Ok(None) => {
                    // Stream ended (Redis disconnected)
                    error!("Redis pub/sub stream ended");
                    return Err("Redis pub/sub stream ended".into());
                }
                Err(_) => {
                    // Timeout — no message received for SUBSCRIBER_TIMEOUT seconds.
                    // With heartbeats every 30s, this should never happen on a healthy connection.
                    // If we get here, the connection is silently dead.
                    error!(
                        "No Redis pub/sub message received for {}s — connection likely dead, forcing reconnect",
                        SUBSCRIBER_TIMEOUT
                    );
                    return Err(
                        "Subscriber timeout — no messages received, connection likely dead".into(),
                    );
                }
            }
        }
    }
}
