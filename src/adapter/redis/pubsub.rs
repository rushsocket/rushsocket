use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

use super::request_response::{Request, RequestType, Response, ResponsePayload};
use crate::config::RedisConfig;

const BROADCAST_SUFFIX: &str = "#broadcast";
const REQUEST_SUFFIX: &str = "#request";
const RESPONSE_SUFFIX: &str = "#response";

/// How often each node publishes a heartbeat through Redis (seconds).
const HEARTBEAT_INTERVAL: u64 = 30;

/// If no message is received on the subscriber for this long, force reconnect (seconds).
/// This catches silently dead TCP connections (load balancer idle timeout, network partition, etc).
const SUBSCRIBER_TIMEOUT: u64 = 90;

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

struct PendingState {
    responses: Vec<ResponsePayload>,
    tx: Option<oneshot::Sender<Vec<ResponsePayload>>>,
    expected_count: usize,
}

pub struct RedisPubSub {
    /// Redis URL for creating connections.
    redis_url: String,
    /// Connection for broadcast publishes (channel messages, heartbeats).
    broadcast_conn: tokio::sync::Mutex<redis::aio::MultiplexedConnection>,
    /// Connection for control-plane publishes (requests, responses).
    /// Separated from broadcast to prevent head-of-line blocking under bursty fan-out.
    control_conn: tokio::sync::Mutex<redis::aio::MultiplexedConnection>,
    /// Channel prefix.
    prefix: String,
    /// Timeout for cross-node requests.
    request_timeout: Duration,
    /// Pending request/response tracking.
    pending: DashMap<String, Arc<tokio::sync::Mutex<PendingState>>>,
    /// Node ID of this instance.
    node_id: String,
    /// Last time any message was received on the subscriber (epoch secs).
    last_received: AtomicU64,
    /// Known peer nodes: node_id -> last-seen instant (replaces PUBSUB NUMSUB).
    known_nodes: DashMap<String, std::time::Instant>,
    /// Whether subscriptions are live and peer discovery is complete.
    subscribed: AtomicBool,
    /// Connection generation counter. Incremented on each reconnect attempt.
    /// Discovery tasks capture the current generation and only set subscribed = true
    /// if it still matches, preventing stale tasks from flipping readiness.
    generation: AtomicU64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl RedisPubSub {
    pub async fn new(config: &RedisConfig, node_id: &str) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(config.url.as_str())?;
        let broadcast_conn = client.get_multiplexed_async_connection().await?;
        let control_conn = client.get_multiplexed_async_connection().await?;

        Ok(Self {
            redis_url: config.url.clone(),
            broadcast_conn: tokio::sync::Mutex::new(broadcast_conn),
            control_conn: tokio::sync::Mutex::new(control_conn),
            prefix: config.prefix.clone(),
            request_timeout: Duration::from_millis(config.request_timeout),
            pending: DashMap::new(),
            node_id: node_id.to_string(),
            last_received: AtomicU64::new(now_secs()),
            known_nodes: DashMap::new(),
            subscribed: AtomicBool::new(false),
            generation: AtomicU64::new(0),
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

        let mut conn = self.broadcast_conn.lock().await;
        match conn.publish::<_, _, ()>(&self.broadcast_channel(), &json).await {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!("Broadcast publish failed, attempting to reconnect: {}", e);
                match redis::Client::open(self.redis_url.as_str()) {
                    Ok(client) => {
                        if let Ok(new_conn) = client.get_multiplexed_async_connection().await {
                            *conn = new_conn;
                            conn.publish::<_, _, ()>(&self.broadcast_channel(), &json).await?;
                            info!("Broadcast publisher reconnected successfully");
                            Ok(())
                        } else {
                            Err(e)
                        }
                    }
                    Err(_) => Err(e),
                }
            }
        }
    }

    /// Send a request to all other nodes and collect responses.
    pub async fn request(
        &self,
        sender_node: &str,
        request_type: RequestType,
    ) -> Result<Vec<ResponsePayload>, redis::RedisError> {
        let request_id = uuid::Uuid::new_v4().to_string();

        // Get known peer count from node registry (no Redis round-trip)
        let expected_count = self.get_peer_count();
        if expected_count == 0 {
            return Ok(vec![]);
        }

        let (tx, rx) = oneshot::channel();

        let state = Arc::new(tokio::sync::Mutex::new(PendingState {
            responses: Vec::new(),
            tx: Some(tx),
            expected_count,
        }));

        self.pending.insert(request_id.clone(), state);

        // Publish the request
        let request = Request {
            request_id: request_id.clone(),
            sender_node: sender_node.to_string(),
            request_type,
        };
        let envelope = PubSubEnvelope {
            sender_node: sender_node.to_string(),
            payload: PubSubPayload::Request(request),
        };
        let json = serde_json::to_string(&envelope).unwrap_or_default();

        {
            let mut conn = self.control_conn.lock().await;
            conn.publish::<_, _, ()>(&self.request_channel(), &json)
                .await?;
        }

        // Wait for responses with timeout
        let timeout = self.request_timeout;
        let result = tokio::time::timeout(timeout, rx).await;

        // Clean up pending
        self.pending.remove(&request_id);

        match result {
            Ok(Ok(responses)) => Ok(responses),
            Ok(Err(_)) => Ok(vec![]),
            Err(_) => {
                debug!("Request {} timed out", request_id);
                Ok(vec![])
            }
        }
    }

    /// Discover peers on startup/reconnect. Publishes unconditionally (bypasses the
    /// known_nodes check) and always waits the full timeout, collecting whatever Ack
    /// responses arrive. Each response envelope populates known_nodes via record_peer().
    pub async fn discover_peers(&self) -> usize {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        // Use expected_count = usize::MAX so the pending state never resolves early —
        // we always wait for the full timeout to collect all possible responses.
        let state = Arc::new(tokio::sync::Mutex::new(PendingState {
            responses: Vec::new(),
            tx: Some(tx),
            expected_count: usize::MAX,
        }));
        self.pending.insert(request_id.clone(), state.clone());

        let request = Request {
            request_id: request_id.clone(),
            sender_node: self.node_id.clone(),
            request_type: RequestType::DiscoverPeers,
        };
        let envelope = PubSubEnvelope {
            sender_node: self.node_id.clone(),
            payload: PubSubPayload::Request(request),
        };
        let json = serde_json::to_string(&envelope).unwrap_or_default();

        // Publish unconditionally — we don't know if peers exist yet
        let publish_result = {
            let mut conn = self.control_conn.lock().await;
            conn.publish::<_, _, ()>(&self.request_channel(), &json).await
        };
        if let Err(e) = publish_result {
            warn!("Failed to publish discover request: {}", e);
            self.pending.remove(&request_id);
            return 0;
        }

        // Wait the full timeout, then count whatever responses arrived
        let _ = tokio::time::timeout(self.request_timeout, rx).await;
        self.pending.remove(&request_id);

        let guard = state.lock().await;
        guard.responses.len()
    }

    /// Handle an incoming response for a pending request.
    async fn handle_response(&self, response: Response) {
        if let Some(state_ref) = self.pending.get(&response.request_id) {
            let state = state_ref.clone();
            let mut guard = state.lock().await;
            guard.responses.push(response.payload);
            if guard.responses.len() >= guard.expected_count {
                if let Some(tx) = guard.tx.take() {
                    let _ = tx.send(guard.responses.clone());
                }
            }
        }
    }

    /// Publish a response to a specific node.
    async fn send_response(
        &self,
        target_node: &str,
        response: Response,
    ) -> Result<(), redis::RedisError> {
        let response_channel = format!("{}{}:{}", self.prefix, RESPONSE_SUFFIX, target_node);
        let envelope = PubSubEnvelope {
            sender_node: self.node_id.clone(),
            payload: PubSubPayload::Response(response),
        };
        let json = serde_json::to_string(&envelope).unwrap_or_default();

        let mut conn = self.control_conn.lock().await;
        conn.publish::<_, _, ()>(&response_channel, &json).await?;
        Ok(())
    }

    /// Record that a peer node is alive (called on any message from that node).
    fn record_peer(&self, node_id: &str) {
        self.known_nodes.insert(node_id.to_string(), std::time::Instant::now());
    }

    /// Get the number of known live peer nodes (excluding self).
    /// Prunes stale entries older than 2x heartbeat interval.
    fn get_peer_count(&self) -> usize {
        let stale_threshold = Duration::from_secs(HEARTBEAT_INTERVAL * 2);
        let now = std::time::Instant::now();
        self.known_nodes.retain(|_, last_seen| now.duration_since(*last_seen) < stale_threshold);
        self.known_nodes.len()
    }

    /// Whether this node's Redis subscriptions are live and peer discovery is complete.
    pub fn is_subscribed(&self) -> bool {
        self.subscribed.load(Ordering::Relaxed)
    }

    /// Get IDs of all known peer nodes (excluding self).
    pub fn get_known_node_ids(&self) -> Vec<String> {
        let stale_threshold = Duration::from_secs(HEARTBEAT_INTERVAL * 2);
        let now = std::time::Instant::now();
        self.known_nodes.retain(|_, last_seen| now.duration_since(*last_seen) < stale_threshold);
        self.known_nodes.iter().map(|e| e.key().clone()).collect()
    }

    /// Subscribe and listen for messages. Runs forever with automatic reconnect.
    pub async fn listen(self: Arc<Self>, adapter: Arc<super::RedisAdapter>) {
        let broadcast_ch = self.broadcast_channel();
        let request_ch = self.request_channel();
        let response_ch = self.response_channel();

        // Spawn the heartbeat publisher — keeps Redis connections alive
        let heartbeat_self = self.clone();
        tokio::spawn(async move {
            heartbeat_self.heartbeat_loop().await;
        });

        // Main subscriber loop with reconnect
        loop {
            // Mark not-ready and bump generation on each (re)connect attempt.
            // Stale discovery tasks from a previous generation will see the mismatch
            // and skip setting subscribed = true.
            self.subscribed.store(false, Ordering::Relaxed);
            self.generation.fetch_add(1, Ordering::Relaxed);
            // Reset last_received before each connection attempt
            self.last_received.store(now_secs(), Ordering::Relaxed);

            match self
                .subscribe_loop(&adapter, &broadcast_ch, &request_ch, &response_ch)
                .await
            {
                Ok(()) => break,
                Err(e) => {
                    // Immediately invalidate readiness so any in-flight discovery
                    // task from this generation cannot flip subscribed back to true
                    // during the sleep window.
                    self.subscribed.store(false, Ordering::Relaxed);
                    self.generation.fetch_add(1, Ordering::Relaxed);
                    error!("Redis subscribe loop error: {}, reconnecting in 1s...", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Periodically publish a heartbeat to keep the Redis pub/sub connection alive.
    /// This ensures traffic flows even when no real events are happening,
    /// preventing silent TCP death from load balancer idle timeouts.
    async fn heartbeat_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(HEARTBEAT_INTERVAL)).await;

            if let Err(e) = self.publish(&self.node_id, &BroadcastMessage::Heartbeat).await {
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
        if let Err(e) = self.publish(&self.node_id, &BroadcastMessage::Heartbeat).await {
            warn!("Failed to send announce heartbeat: {}", e);
        }

        // Discover existing peers: publish unconditionally and wait for the full timeout
        // to collect responses. Each response populates known_nodes via record_peer().
        // Spawned concurrently so the message loop can process inbound responses.
        let discover_self = self.clone();
        let generation = self.generation.load(Ordering::Relaxed);
        tokio::spawn(async move {
            let count = discover_self.discover_peers().await;
            info!("Peer discovery complete: {} peer(s) found", count);
            // Only set ready if this is still the current generation.
            // If a reconnect happened while we were waiting, our generation is stale.
            if discover_self.generation.load(Ordering::Relaxed) == generation {
                discover_self.subscribed.store(true, Ordering::Relaxed);
                info!("Redis adapter ready (generation {})", generation);
            } else {
                info!("Discovery result discarded (generation {} superseded)", generation);
            }
        });

        let mut msg_stream = pubsub.on_message();

        use futures_util::StreamExt;

        loop {
            // Wait for next message OR timeout if nothing received for too long.
            // This catches silently dead connections that never error — they just
            // stop delivering messages.
            let msg = tokio::time::timeout(
                Duration::from_secs(SUBSCRIBER_TIMEOUT),
                msg_stream.next(),
            )
            .await;

            match msg {
                Ok(Some(msg)) => {
                    // Got a message — record activity
                    self.last_received.store(now_secs(), Ordering::Relaxed);

                    let payload: String = match msg.get_payload() {
                        Ok(p) => p,
                        Err(e) => {
                            warn!("Failed to get message payload: {}", e);
                            continue;
                        }
                    };

                    let envelope: PubSubEnvelope = match serde_json::from_str(&payload) {
                        Ok(e) => e,
                        Err(e) => {
                            warn!("Failed to deserialize pub/sub message: {}", e);
                            continue;
                        }
                    };

                    // Skip own messages (except we still record them as activity)
                    if envelope.sender_node == self.node_id {
                        continue;
                    }

                    // Track this peer as alive
                    self.record_peer(&envelope.sender_node);

                    match envelope.payload {
                        PubSubPayload::Broadcast(BroadcastMessage::Heartbeat) => {
                            // Heartbeat from another node — just keep-alive, no action needed
                            debug!("Heartbeat received from {}", envelope.sender_node);
                        }
                        PubSubPayload::Broadcast(broadcast) => {
                            adapter
                                .handle_broadcast(&envelope.sender_node, &broadcast)
                                .await;
                        }
                        PubSubPayload::Request(request) => {
                            let response_payload = adapter.handle_request(&request).await;
                            let response = Response {
                                request_id: request.request_id,
                                responder_node: self.node_id.clone(),
                                payload: response_payload,
                            };
                            if let Err(e) =
                                self.send_response(&request.sender_node, response).await
                            {
                                warn!("Failed to send response: {}", e);
                            }
                        }
                        PubSubPayload::Response(response) => {
                            self.handle_response(response).await;
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
                    return Err("Subscriber timeout — no messages received, connection likely dead".into());
                }
            }
        }
    }
}
