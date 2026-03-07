pub mod pubsub;
pub mod request_response;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use redis::AsyncCommands;
use tracing::{error, warn};

use super::local::LocalAdapter;
use super::{Adapter, ChannelInfo};
use crate::config::RedisConfig;
use crate::protocol::channels::ChannelKind;
use crate::websocket::connection::WsConnection;

use pubsub::RedisPubSub;
use request_response::{RequestType, ResponsePayload};

pub struct RedisAdapter {
    local: LocalAdapter,
    pubsub: Arc<RedisPubSub>,
    node_id: String,
    prefix: String,
    cmd_conn: tokio::sync::Mutex<redis::aio::MultiplexedConnection>,
    metrics: std::sync::OnceLock<Arc<crate::metrics::Metrics>>,
}

impl RedisAdapter {
    pub async fn new(config: &RedisConfig) -> Result<Self, redis::RedisError> {
        let node_id = uuid::Uuid::new_v4().to_string();
        let pubsub = Arc::new(RedisPubSub::new(config, &node_id).await?);
        let client = redis::Client::open(config.url.as_str())?;
        let cmd_conn = client.get_multiplexed_async_connection().await?;

        Ok(Self {
            local: LocalAdapter::new(),
            pubsub,
            node_id,
            prefix: config.prefix.clone(),
            cmd_conn: tokio::sync::Mutex::new(cmd_conn),
            metrics: std::sync::OnceLock::new(),
        })
    }

    fn occupancy_key(&self, app_id: &str, channel: &str) -> String {
        format!("{}:occupied:{}:{}", self.prefix, app_id, channel)
    }

    fn presence_key(&self, app_id: &str, channel: &str) -> String {
        format!("{}:presence:{}:{}", self.prefix, app_id, channel)
    }

    fn presence_sockets_key(&self, app_id: &str, channel: &str) -> String {
        format!("{}:psock:{}:{}", self.prefix, app_id, channel)
    }

    fn node_stats_key(&self, node_id: &str) -> String {
        format!("{}:node_stats:{}", self.prefix, node_id)
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Set the metrics reference (called after AppState creation).
    pub fn set_metrics(&self, metrics: Arc<crate::metrics::Metrics>) {
        let _ = self.metrics.set(metrics);
    }

    /// Get aggregated metrics from all nodes (local + remote).
    pub async fn get_aggregated_metrics(&self) -> serde_json::Value {
        let local_metrics = self.metrics.get()
            .map(|m| m.to_json())
            .unwrap_or_else(|| serde_json::json!({}));

        let request = RequestType::GetMetrics;
        match self.pubsub.request(&self.node_id, request).await {
            Ok(responses) => {
                let mut merged = local_metrics;
                for r in responses {
                    if let ResponsePayload::Metrics(remote) = r {
                        merge_metrics(&mut merged, &remote);
                    }
                }
                merged
            }
            Err(e) => {
                warn!("Failed to query peer metrics: {}", e);
                local_metrics
            }
        }
    }

    /// Collect local stats for this node (per-app connection counts and channel socket counts).
    async fn collect_local_stats(&self) -> serde_json::Value {
        let app_ids = self.local.get_app_ids();
        let mut apps = serde_json::Map::new();
        for app_id in app_ids {
            let connections = self.local.get_connection_count(&app_id).await;
            let channels = self.local.get_channels(&app_id).await;
            let mut ch_map = serde_json::Map::new();
            for ch in &channels {
                let count = self.local.get_channel_socket_count(&app_id, ch).await;
                ch_map.insert(ch.clone(), serde_json::json!(count));
            }
            apps.insert(app_id, serde_json::json!({
                "connections": connections,
                "channels": ch_map,
            }));
        }
        serde_json::json!(apps)
    }

    /// Read stats from all known remote nodes via Redis.
    async fn read_remote_stats(&self) -> Vec<serde_json::Value> {
        let node_ids = self.pubsub.get_known_node_ids();
        if node_ids.is_empty() {
            return vec![];
        }
        let keys: Vec<String> = node_ids.iter()
            .map(|id| self.node_stats_key(id))
            .collect();
        let mut conn = self.cmd_conn.lock().await;
        let values: Vec<Option<String>> = match conn.mget(&keys).await {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to read remote stats from Redis: {}", e);
                return vec![];
            }
        };
        values.into_iter()
            .filter_map(|v| v.and_then(|s| serde_json::from_str(&s).ok()))
            .collect()
    }

    /// Background loop: write local stats to Redis every 5 seconds with 15s TTL.
    async fn stats_heartbeat_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let stats = self.collect_local_stats().await;
            let json = serde_json::to_string(&stats).unwrap_or_default();
            let key = self.node_stats_key(&self.node_id);
            let mut conn = self.cmd_conn.lock().await;
            let result: Result<(), redis::RedisError> = redis::cmd("SET")
                .arg(&key)
                .arg(&json)
                .arg("EX")
                .arg(15i64)
                .query_async(&mut *conn)
                .await;
            if let Err(e) = result {
                warn!("Failed to write stats heartbeat: {}", e);
            }
        }
    }

    /// Start the subscription listener loop. Must be called after construction.
    pub async fn start_listening(self: &Arc<Self>) {
        let adapter = self.clone();
        let pubsub = self.pubsub.clone();
        tokio::spawn(async move {
            pubsub.listen(adapter).await;
        });

        // Stats heartbeat: write local stats to Redis periodically
        let adapter2 = self.clone();
        tokio::spawn(async move {
            adapter2.stats_heartbeat_loop().await;
        });
    }

    /// Handle an incoming broadcast message from another node.
    pub async fn handle_broadcast(&self, sender_node: &str, payload: &pubsub::BroadcastMessage) {
        if sender_node == self.node_id {
            return; // Skip own messages
        }

        match payload {
            pubsub::BroadcastMessage::ChannelMessage {
                app_id,
                channel,
                message,
                excepting_id,
            } => {
                // Deliver to local sockets (String→Bytes is zero-copy ownership transfer)
                self.local
                    .send_to_channel(app_id, channel, Bytes::from(message.clone()), excepting_id.as_deref())
                    .await;
            }
            pubsub::BroadcastMessage::TerminateUser { app_id, user_id } => {
                self.local
                    .terminate_user_connections(app_id, user_id)
                    .await;
            }
            pubsub::BroadcastMessage::Heartbeat => {
                // No-op — heartbeats are handled in the subscriber loop
            }
        }
    }

    /// Handle an incoming request from another node and produce a response.
    pub async fn handle_request(
        &self,
        request: &request_response::Request,
    ) -> ResponsePayload {
        match &request.request_type {
            RequestType::GetConnectionCount { app_id } => {
                let count = self.local.get_connection_count(app_id).await;
                ResponsePayload::Count(count)
            }
            RequestType::GetChannels { app_id } => {
                let channels = self.local.get_channels(app_id).await;
                ResponsePayload::Channels(channels)
            }
            RequestType::GetChannelSocketCount { app_id, channel } => {
                let count = self.local.get_channel_socket_count(app_id, channel).await;
                ResponsePayload::Count(count)
            }
            RequestType::GetChannelInfo { app_id, channel } => {
                let info = self.local.get_channel_info(app_id, channel).await;
                ResponsePayload::ChannelInfo {
                    occupied: info.occupied,
                    subscription_count: info.subscription_count,
                    user_count: info.user_count,
                }
            }
            RequestType::GetPresenceMembers { app_id, channel } => {
                let members = self.local.get_presence_members(app_id, channel).await;
                ResponsePayload::PresenceMembers(members)
            }
            RequestType::GetPresenceUserCount { app_id, channel } => {
                let count = self.local.get_presence_user_count(app_id, channel).await;
                ResponsePayload::Count(count)
            }
            RequestType::GetChannelSockets { app_id, channel } => {
                let sockets = self.local.get_channel_sockets(app_id, channel).await;
                ResponsePayload::SocketIds(sockets)
            }
            RequestType::GetMetrics => {
                let metrics_json = self.metrics.get()
                    .map(|m| m.to_json())
                    .unwrap_or_else(|| serde_json::json!({}));
                ResponsePayload::Metrics(metrics_json)
            }
            RequestType::DiscoverPeers => {
                ResponsePayload::Ack
            }
        }
    }
}

/// Merge remote metrics into local metrics by summing numeric fields.
fn merge_metrics(local: &mut serde_json::Value, remote: &serde_json::Value) {
    if let (Some(l), Some(r)) = (local.as_object_mut(), remote.as_object()) {
        for (key, remote_val) in r {
            if key == "apps" {
                // Merge per-app metrics
                if let (Some(l_apps), Some(r_apps)) = (
                    l.entry(key).or_insert_with(|| serde_json::json!({})).as_object_mut(),
                    remote_val.as_object(),
                ) {
                    for (app_id, r_app) in r_apps {
                        let l_app = l_apps.entry(app_id).or_insert_with(|| serde_json::json!({}));
                        merge_metrics(l_app, r_app);
                    }
                }
            } else if let (Some(l_val), Some(r_num)) = (l.get_mut(key), remote_val.as_i64()) {
                if let Some(l_num) = l_val.as_i64() {
                    *l_val = serde_json::json!(l_num + r_num);
                }
            } else if remote_val.is_number() {
                l.entry(key).or_insert(remote_val.clone());
            }
        }
    }
}

#[async_trait]
impl Adapter for RedisAdapter {
    async fn add_socket(&self, app_id: &str, conn: Arc<WsConnection>) {
        self.local.add_socket(app_id, conn).await;
    }

    async fn try_add_socket(&self, app_id: &str, conn: Arc<WsConnection>, limit: i64) -> bool {
        self.local.try_add_socket(app_id, conn, limit).await
    }

    async fn remove_socket(&self, app_id: &str, socket_id: &str) -> Vec<String> {
        self.local.remove_socket(app_id, socket_id).await
    }

    async fn add_to_channel(&self, app_id: &str, socket_id: &str, channel: &str) -> bool {
        self.local.add_to_channel(app_id, socket_id, channel).await
    }

    async fn remove_from_channel(&self, app_id: &str, socket_id: &str, channel: &str) -> bool {
        self.local
            .remove_from_channel(app_id, socket_id, channel)
            .await
    }

    async fn send_to_channel(
        &self,
        app_id: &str,
        channel: &str,
        message: Bytes,
        excepting_id: Option<&str>,
    ) {
        // Broadcast to other nodes (extract &str from Bytes — zero-copy since all messages are UTF-8)
        let message_str = std::str::from_utf8(&message).unwrap_or_default();
        let broadcast = pubsub::BroadcastMessage::ChannelMessage {
            app_id: app_id.to_string(),
            channel: channel.to_string(),
            message: message_str.to_string(),
            excepting_id: excepting_id.map(|s| s.to_string()),
        };
        if let Err(e) = self.pubsub.publish(&self.node_id, &broadcast).await {
            error!("Failed to broadcast to Redis: {}", e);
        }

        // Send to local sockets (Bytes refcount shared across recipients)
        self.local
            .send_to_channel(app_id, channel, message, excepting_id)
            .await;
    }

    async fn get_connection_count(&self, app_id: &str) -> usize {
        let local_count = self.local.get_connection_count(app_id).await;
        let remote_stats = self.read_remote_stats().await;
        let remote_count: usize = remote_stats.iter()
            .filter_map(|s| s.get(app_id))
            .filter_map(|a| a["connections"].as_u64())
            .map(|c| c as usize)
            .sum();
        local_count + remote_count
    }

    async fn get_channels(&self, app_id: &str) -> Vec<String> {
        let mut all_channels: std::collections::HashSet<String> =
            self.local.get_channels(app_id).await.into_iter().collect();
        let remote_stats = self.read_remote_stats().await;
        for stats in &remote_stats {
            if let Some(app) = stats.get(app_id) {
                if let Some(channels) = app["channels"].as_object() {
                    all_channels.extend(channels.keys().cloned());
                }
            }
        }
        all_channels.into_iter().collect()
    }

    async fn get_channel_info(&self, app_id: &str, channel: &str) -> ChannelInfo {
        let subscription_count = self.get_channel_socket_count(app_id, channel).await;
        let kind = ChannelKind::from_name(channel);
        let user_count = if kind.is_presence() {
            Some(self.get_presence_user_count(app_id, channel).await)
        } else {
            None
        };
        ChannelInfo {
            occupied: subscription_count > 0,
            subscription_count,
            user_count,
        }
    }

    async fn get_channel_socket_count(&self, app_id: &str, channel: &str) -> usize {
        let local_count = self.local.get_channel_socket_count(app_id, channel).await;
        let remote_stats = self.read_remote_stats().await;
        let remote_count: usize = remote_stats.iter()
            .filter_map(|s| s.get(app_id))
            .filter_map(|a| a["channels"].as_object())
            .filter_map(|ch| ch.get(channel))
            .filter_map(|c| c.as_u64())
            .map(|c| c as usize)
            .sum();
        local_count + remote_count
    }

    async fn get_presence_members(
        &self,
        app_id: &str,
        channel: &str,
    ) -> HashMap<String, serde_json::Value> {
        let presence_key = self.presence_key(app_id, channel);
        let mut conn = self.cmd_conn.lock().await;
        let raw: HashMap<String, String> = match conn.hgetall(&presence_key).await {
            Ok(m) => m,
            Err(e) => {
                warn!("Redis HGETALL for presence failed: {}", e);
                drop(conn);
                return self.local.get_presence_members(app_id, channel).await;
            }
        };
        raw.into_iter()
            .map(|(uid, info_str)| {
                let info = serde_json::from_str(&info_str).unwrap_or(serde_json::Value::Null);
                (uid, info)
            })
            .collect()
    }

    async fn add_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: &serde_json::Value,
    ) -> bool {
        // Local tracking for socket-level disconnect cleanup
        self.local
            .add_presence_member(app_id, channel, socket_id, user_id, user_info)
            .await;

        // Global tracking via Redis: increment socket count for this user
        let psock_key = self.presence_sockets_key(app_id, channel);
        let mut conn = self.cmd_conn.lock().await;
        let count: i64 = match conn.hincr(&psock_key, user_id, 1i64).await {
            Ok(c) => c,
            Err(e) => {
                warn!("Redis HINCRBY for presence add failed: {}", e);
                return false;
            }
        };
        let is_new = count == 1;
        if is_new {
            let presence_key = self.presence_key(app_id, channel);
            let info_json = serde_json::to_string(user_info).unwrap_or_default();
            if let Err(e) = conn.hset::<_, _, _, ()>(&presence_key, user_id, &info_json).await {
                warn!("Redis HSET for presence member failed: {}", e);
            }
        }
        is_new
    }

    async fn try_add_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: &serde_json::Value,
        max_members: usize,
    ) -> Result<(bool, HashMap<String, serde_json::Value>), ()> {
        let presence_key = self.presence_key(app_id, channel);
        let psock_key = self.presence_sockets_key(app_id, channel);

        // Check global member limit via Redis
        let mut conn = self.cmd_conn.lock().await;
        let current_count: usize = conn.hlen(&presence_key).await.unwrap_or(0);
        let already_exists: bool = conn.hexists(&presence_key, user_id).await.unwrap_or(false);
        if !already_exists && current_count >= max_members {
            return Err(());
        }

        // Local add for socket tracking (ignore local is_new, use Redis-based one)
        let _ = self
            .local
            .add_presence_member(app_id, channel, socket_id, user_id, user_info)
            .await;

        // Redis: increment socket count and store user info if new
        let count: i64 = conn.hincr(&psock_key, user_id, 1i64).await.unwrap_or(1);
        let is_new = count == 1;
        if is_new {
            let info_json = serde_json::to_string(user_info).unwrap_or_default();
            let _ = conn.hset::<_, _, _, ()>(&presence_key, user_id, &info_json).await;
        }
        drop(conn);

        // Get members from Redis for subscription_succeeded response
        let members = self.get_presence_members(app_id, channel).await;
        Ok((is_new, members))
    }

    async fn remove_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
    ) -> bool {
        // Local remove (tracks per-socket state)
        self.local
            .remove_presence_member(app_id, channel, socket_id, user_id)
            .await;

        // Global remove via Redis: decrement socket count
        let psock_key = self.presence_sockets_key(app_id, channel);
        let mut conn = self.cmd_conn.lock().await;
        let count: i64 = match conn.hincr(&psock_key, user_id, -1i64).await {
            Ok(c) => c,
            Err(e) => {
                warn!("Redis HINCRBY for presence remove failed: {}", e);
                return false;
            }
        };
        if count <= 0 {
            // User has no more sockets globally — remove from presence hash
            let presence_key = self.presence_key(app_id, channel);
            let _ = conn.hdel::<_, _, ()>(&presence_key, user_id).await;
            let _ = conn.hdel::<_, _, ()>(&psock_key, user_id).await;

            // Clean up empty hashes
            let remaining: usize = conn.hlen(&presence_key).await.unwrap_or(1);
            if remaining == 0 {
                let _ = conn.del::<_, ()>(&presence_key).await;
                let _ = conn.del::<_, ()>(&psock_key).await;
            }
            return true;
        }
        false
    }

    async fn get_presence_user_count(&self, app_id: &str, channel: &str) -> usize {
        let presence_key = self.presence_key(app_id, channel);
        let mut conn = self.cmd_conn.lock().await;
        match conn.hlen(&presence_key).await {
            Ok(count) => count,
            Err(e) => {
                warn!("Redis HLEN for presence count failed: {}", e);
                drop(conn);
                self.local.get_presence_user_count(app_id, channel).await
            }
        }
    }

    async fn get_sockets_for_user(
        &self,
        app_id: &str,
        user_id: &str,
    ) -> Vec<Arc<WsConnection>> {
        // Only return local sockets (can't return remote Arc<WsConnection>)
        self.local.get_sockets_for_user(app_id, user_id).await
    }

    async fn terminate_user_connections(&self, app_id: &str, user_id: &str) {
        // Terminate locally
        self.local
            .terminate_user_connections(app_id, user_id)
            .await;

        // Broadcast to other nodes
        let broadcast = pubsub::BroadcastMessage::TerminateUser {
            app_id: app_id.to_string(),
            user_id: user_id.to_string(),
        };
        if let Err(e) = self.pubsub.publish(&self.node_id, &broadcast).await {
            error!("Failed to broadcast terminate to Redis: {}", e);
        }
    }

    async fn get_socket(
        &self,
        app_id: &str,
        socket_id: &str,
    ) -> Option<Arc<WsConnection>> {
        self.local.get_socket(app_id, socket_id).await
    }

    async fn get_aggregated_metrics(&self) -> Option<serde_json::Value> {
        Some(RedisAdapter::get_aggregated_metrics(self).await)
    }

    async fn get_channel_sockets(&self, app_id: &str, channel: &str) -> Vec<String> {
        let mut all_sockets: std::collections::HashSet<String> = self
            .local
            .get_channel_sockets(app_id, channel)
            .await
            .into_iter()
            .collect();

        let request = RequestType::GetChannelSockets {
            app_id: app_id.to_string(),
            channel: channel.to_string(),
        };
        if let Ok(responses) = self.pubsub.request(&self.node_id, request).await {
            for r in responses {
                if let ResponsePayload::SocketIds(ids) = r {
                    all_sockets.extend(ids);
                }
            }
        }

        all_sockets.into_iter().collect()
    }

    async fn get_local_connection_count(&self, app_id: &str) -> usize {
        self.local.get_connection_count(app_id).await
    }

    async fn get_local_channels(&self, app_id: &str) -> Vec<String> {
        self.local.get_channels(app_id).await
    }

    async fn get_local_presence_user_count(&self, app_id: &str, channel: &str) -> usize {
        self.local.get_presence_user_count(app_id, channel).await
    }

    async fn add_user(&self, app_id: &str, user_id: &str, socket_id: &str) {
        self.local.add_user(app_id, user_id, socket_id).await;
    }

    async fn remove_user(&self, app_id: &str, user_id: &str, socket_id: &str) {
        self.local.remove_user(app_id, user_id, socket_id).await;
    }

    async fn is_channel_globally_occupied(
        &self,
        app_id: &str,
        channel: &str,
        locally_occupied: bool,
    ) -> bool {
        if !locally_occupied {
            return false;
        }
        let key = self.occupancy_key(app_id, channel);
        let mut conn = self.cmd_conn.lock().await;
        // SETNX returns true only if the key was newly set (first node to claim occupancy)
        match conn.set_nx::<_, _, bool>(&key, &self.node_id).await {
            Ok(was_set) => was_set,
            Err(e) => {
                warn!("Redis SETNX for channel occupancy failed: {}", e);
                // Fallback to local state on Redis error
                locally_occupied
            }
        }
    }

    async fn is_channel_globally_vacated(
        &self,
        app_id: &str,
        channel: &str,
        locally_vacated: bool,
    ) -> bool {
        if !locally_vacated {
            return false;
        }
        // Real-time check: local + broadcast to peers for actual socket count.
        // We cannot use get_channel_socket_count here because it uses stats heartbeat
        // data which may be stale. Vacated checks require real-time accuracy.
        let local_count = self.local.get_channel_socket_count(app_id, channel).await;
        if local_count > 0 {
            return false;
        }
        let request = RequestType::GetChannelSocketCount {
            app_id: app_id.to_string(),
            channel: channel.to_string(),
        };
        if let Ok(responses) = self.pubsub.request(&self.node_id, request).await {
            let remote: usize = responses
                .iter()
                .filter_map(|r| match r {
                    ResponsePayload::Count(c) => Some(*c),
                    _ => None,
                })
                .sum();
            if remote > 0 {
                return false;
            }
        }
        // No subscribers anywhere — clean up the occupancy key
        let key = self.occupancy_key(app_id, channel);
        let mut conn = self.cmd_conn.lock().await;
        if let Err(e) = conn.del::<_, ()>(&key).await {
            warn!("Redis DEL for channel occupancy failed: {}", e);
        }
        true
    }

    fn is_ready(&self) -> bool {
        self.pubsub.is_subscribed()
    }
}
