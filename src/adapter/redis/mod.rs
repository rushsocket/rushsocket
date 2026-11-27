pub mod lua;
mod presence;
pub mod pubsub;
pub mod request_response;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use redis::AsyncCommands;
use tracing::{error, warn};

use lua::QUOTA_LUA;

use super::local::LocalAdapter;
use super::{Adapter, Admit, ChannelInfo, map_quota_eval};

const OCCUPANCY_TTL_SECS: i64 = 15;

/// Vacated iff no known peers, or every expected peer answered and the count sum is 0.
pub(crate) fn vacated_from_peer_rpc(expected: usize, received: usize, remote_sum: usize) -> bool {
    expected == 0 || (received == expected && remote_sum == 0)
}
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
    cmd_conn: redis::aio::ConnectionManager,
    commands_ready: AtomicBool,
    heartbeat_lock: tokio::sync::Mutex<()>,
    presence_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    /// Serialize same-node quota admits/removes per app (snapshot → Lua → local).
    quota_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

impl RedisAdapter {
    pub async fn new(
        config: &RedisConfig,
        metrics: Arc<crate::metrics::Metrics>,
    ) -> Result<Self, redis::RedisError> {
        let node_id = uuid::Uuid::new_v4().to_string();
        let pubsub = Arc::new(RedisPubSub::new(config, &node_id, metrics).await?);
        let client = redis::Client::open(config.url.as_str())?;
        let cmd_conn = client.get_connection_manager().await?;

        Ok(Self {
            local: LocalAdapter::new(),
            pubsub,
            node_id,
            prefix: config.prefix.clone(),
            cmd_conn,
            commands_ready: AtomicBool::new(false),
            heartbeat_lock: tokio::sync::Mutex::new(()),
            presence_locks: DashMap::new(),
            quota_locks: DashMap::new(),
        })
    }

    fn connections_key(&self, app_id: &str) -> String {
        format!("{}:connections:{}", self.prefix, app_id)
    }

    fn quota_lock(&self, app_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.quota_locks
            .entry(app_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Caller must hold `quota_lock(app_id)` when `set_count` is `Some` (SET path).
    async fn quota_eval(
        &self,
        app_id: &str,
        set_count: Option<i64>,
        limit: i64,
    ) -> Result<(bool, i64), redis::RedisError> {
        let key = self.connections_key(app_id);
        let stats_prefix = format!("{}:node_stats:", self.prefix);
        let mode = if set_count.is_some() { 1i64 } else { 0 };
        let new_count = set_count.unwrap_or(0);
        let mut conn = self.cmd_conn.clone();
        let result: Vec<i64> = redis::cmd("EVAL")
            .arg(QUOTA_LUA)
            .arg(1)
            .arg(&key)
            .arg(&self.node_id)
            .arg(new_count)
            .arg(limit)
            .arg(mode)
            .arg(&stats_prefix)
            .query_async(&mut conn)
            .await?;
        let ok = result.first().copied().unwrap_or(0) == 1;
        let sum = result.get(1).copied().unwrap_or(0);
        Ok((ok, sum))
    }

    fn presence_lock(&self, app: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.presence_locks
            .entry(app.to_owned())
            .or_default()
            .clone()
    }

    fn occupancy_key(&self, app_id: &str, channel: &str) -> String {
        format!("{}:occupied:{}:{}", self.prefix, app_id, channel)
    }

    fn node_stats_key(&self, node_id: &str) -> String {
        format!("{}:node_stats:{}", self.prefix, node_id)
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
            apps.insert(
                app_id,
                serde_json::json!({
                    "connections": connections,
                    "channels": ch_map,
                }),
            );
        }
        serde_json::json!(apps)
    }

    /// Read stats from all known remote nodes via Redis.
    async fn read_remote_stats(&self) -> Vec<serde_json::Value> {
        let node_ids = self.pubsub.get_known_node_ids();
        if node_ids.is_empty() {
            return vec![];
        }
        let keys: Vec<String> = node_ids.iter().map(|id| self.node_stats_key(id)).collect();
        let mut conn = self.cmd_conn.clone();
        let values: Vec<Option<String>> = match conn.mget(&keys).await {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to read remote stats from Redis: {}", e);
                return vec![];
            }
        };
        values
            .into_iter()
            .filter_map(|v| v.and_then(|s| serde_json::from_str(&s).ok()))
            .collect()
    }

    /// Background loop: write local stats to Redis every 5 seconds with 15s TTL.
    /// Writes immediately on start so peers cannot reap this node's quota field
    /// during the first 5s window.
    async fn refresh_heartbeat(&self) -> Result<(), redis::RedisError> {
        let _guard = self.heartbeat_lock.lock().await;
        let result = self.write_heartbeat().await;
        self.commands_ready.store(result.is_ok(), Ordering::Release);
        result
    }

    async fn write_heartbeat(&self) -> Result<(), redis::RedisError> {
        let stats = self.collect_local_stats().await;
        let mut conn = self.cmd_conn.clone();
        conn.set_ex::<_, _, ()>(self.node_stats_key(&self.node_id), stats.to_string(), 15)
            .await?;
        for app in self.local.get_app_ids() {
            let channels = self.local.get_channels(&app).await;
            self.refresh_presence(&app, &channels).await?;
            let mut pipe = redis::pipe();
            for ch in channels {
                pipe.cmd("EXPIRE")
                    .arg(self.occupancy_key(&app, &ch))
                    .arg(OCCUPANCY_TTL_SECS)
                    .ignore();
            }
            if !pipe.is_empty() {
                pipe.query_async::<()>(&mut conn).await?;
            }
            let lock = self.quota_lock(&app);
            let _guard = lock.lock().await;
            let count = self.local.get_connection_count(&app).await as i64;
            self.quota_eval(&app, Some(count), -1).await?;
        }
        Ok(())
    }

    async fn stats_heartbeat_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Err(e) = self.refresh_heartbeat().await {
                warn!("Redis state heartbeat failed: {e}");
            }
        }
    }

    /// Start the subscription listener loop. Must be called after construction.
    pub async fn start_listening(self: &Arc<Self>) {
        let heartbeat = self.pubsub.clone();
        tokio::spawn(async move {
            heartbeat.heartbeat_loop().await;
        });

        let adapter = self.clone();
        let pubsub = self.pubsub.clone();
        tokio::spawn(async move {
            loop {
                pubsub.clone().listen(adapter.clone()).await;
                warn!("Redis pub/sub listen exited; restarting");
            }
        });

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
                    .send_to_channel(
                        app_id,
                        channel,
                        Bytes::from(message.clone()),
                        excepting_id.as_deref(),
                    )
                    .await;
            }
            pubsub::BroadcastMessage::TerminateUser { app_id, user_id } => {
                self.local.terminate_user_connections(app_id, user_id).await;
            }
            pubsub::BroadcastMessage::Heartbeat => {
                // No-op — heartbeats are handled in the subscriber loop
            }
        }
    }

    /// Handle an incoming request from another node and produce a response.
    pub async fn handle_request(&self, request: &request_response::Request) -> ResponsePayload {
        match &request.request_type {
            RequestType::GetChannelSocketCount { app_id, channel } => {
                let count = self.local.get_channel_socket_count(app_id, channel).await;
                ResponsePayload::Count { count }
            }
            RequestType::TerminateUser { app_id, user_id } => {
                self.local.terminate_user_connections(app_id, user_id).await;
                ResponsePayload::Ack
            }
            RequestType::DiscoverPeers => ResponsePayload::Ack,
        }
    }
}

#[async_trait]
impl Adapter for RedisAdapter {
    async fn add_socket(&self, app_id: &str, conn: Arc<WsConnection>) {
        self.local.add_socket(app_id, conn).await;
    }

    async fn try_add_socket(&self, app_id: &str, conn: Arc<WsConnection>, limit: i64) -> Admit {
        let lock = self.quota_lock(app_id);
        let _g = lock.lock().await;
        let local_count = self.local.get_connection_count(app_id).await as i64;
        let next = local_count + 1;
        match self.quota_eval(app_id, Some(next), limit).await {
            Ok((ok, _)) => match map_quota_eval(limit, Ok(ok)) {
                Admit::Ok => {}
                other => return other,
            },
            Err(e) => {
                warn!("Redis quota EVAL failed: {e}");
                match map_quota_eval(limit, Err(())) {
                    Admit::Ok => {}
                    other => return other,
                }
            }
        }
        match self.local.try_add_socket(app_id, conn, -1).await {
            Admit::Ok => Admit::Ok,
            other => {
                if let Err(e) = self.quota_eval(app_id, Some(local_count), -1).await {
                    warn!("Redis quota rollback failed: {e}");
                }
                other
            }
        }
    }

    async fn remove_socket(&self, app_id: &str, socket_id: &str) -> Vec<String> {
        let lock = self.quota_lock(app_id);
        let _g = lock.lock().await;
        let vacated = self.local.remove_socket(app_id, socket_id).await;
        let count = self.local.get_connection_count(app_id).await as i64;
        if let Err(e) = self.quota_eval(app_id, Some(count), -1).await {
            warn!("Redis quota HSET on remove failed: {e}");
        }
        vacated
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
        if let Err(e) = self
            .try_send_to_channel(app_id, channel, message, excepting_id)
            .await
        {
            error!("Channel publish failed: {e}");
        }
    }

    async fn try_send_to_channel(
        &self,
        app_id: &str,
        channel: &str,
        message: Bytes,
        excepting_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Broadcast to other nodes (extract &str from Bytes — zero-copy since all messages are UTF-8)
        let message_str = std::str::from_utf8(&message).unwrap_or_default();
        let broadcast = pubsub::BroadcastMessage::ChannelMessage {
            app_id: app_id.to_string(),
            channel: channel.to_string(),
            message: message_str.to_string(),
            excepting_id: excepting_id.map(|s| s.to_string()),
        };
        let published = self.pubsub.publish(&self.node_id, &broadcast).await;
        // Send to local sockets (Bytes refcount shared across recipients)
        self.local
            .send_to_channel(app_id, channel, message, excepting_id)
            .await;
        Ok(published?)
    }

    async fn get_connection_count(&self, app_id: &str) -> usize {
        match self.quota_eval(app_id, None, -1).await {
            Ok((_, sum)) => sum.max(0) as usize,
            Err(e) => {
                warn!("Redis quota GET failed: {e}");
                self.local.get_connection_count(app_id).await
            }
        }
    }

    async fn get_channels(&self, app_id: &str) -> Vec<String> {
        let mut all_channels: std::collections::HashSet<String> =
            self.local.get_channels(app_id).await.into_iter().collect();
        let remote_stats = self.read_remote_stats().await;
        for stats in &remote_stats {
            if let Some(app) = stats.get(app_id)
                && let Some(channels) = app["channels"].as_object()
            {
                all_channels.extend(channels.keys().cloned());
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
        self.get_channel_socket_counts(app_id, &[channel.to_string()])
            .await
            .get(channel)
            .copied()
            .unwrap_or(0)
    }

    async fn get_channel_socket_counts(
        &self,
        app_id: &str,
        channels: &[String],
    ) -> HashMap<String, usize> {
        let remote_stats = self.read_remote_stats().await;
        let mut out = HashMap::with_capacity(channels.len());
        for ch in channels {
            let local_count = self.local.get_channel_socket_count(app_id, ch).await;
            let remote_count: usize = remote_stats
                .iter()
                .filter_map(|s| s.get(app_id))
                .filter_map(|a| a["channels"].as_object())
                .filter_map(|m| m.get(ch))
                .filter_map(|c| c.as_u64())
                .map(|c| c as usize)
                .sum();
            out.insert(ch.clone(), local_count + remote_count);
        }
        out
    }

    async fn get_presence_members(
        &self,
        app_id: &str,
        channel: &str,
    ) -> HashMap<String, serde_json::Value> {
        match self.read_members(app_id, channel).await {
            Ok(members) => members,
            Err(e) => {
                warn!("Redis presence read failed: {e}");
                self.local.get_presence_members(app_id, channel).await
            }
        }
    }

    async fn add_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: &serde_json::Value,
    ) -> bool {
        match self
            .try_add_presence_member(app_id, channel, socket_id, user_id, user_info, usize::MAX)
            .await
        {
            Ok((is_new, _)) => is_new,
            Err(()) => false,
        }
    }

    #[allow(clippy::result_unit_err)]
    async fn try_add_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: &serde_json::Value,
        max_members: usize,
    ) -> Result<(bool, HashMap<String, serde_json::Value>), ()> {
        self.join_presence(app_id, channel, socket_id, user_id, user_info, max_members)
            .await
    }

    async fn remove_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
    ) -> bool {
        self.leave_presence(app_id, channel, socket_id, user_id)
            .await
    }

    async fn get_presence_user_count(&self, app_id: &str, channel: &str) -> usize {
        self.get_presence_members(app_id, channel).await.len()
    }

    async fn get_sockets_for_user(&self, app_id: &str, user_id: &str) -> Vec<Arc<WsConnection>> {
        // Only return local sockets (can't return remote Arc<WsConnection>)
        self.local.get_sockets_for_user(app_id, user_id).await
    }

    async fn terminate_user_connections(&self, app_id: &str, user_id: &str) {
        if let Err(e) = self.try_terminate_user_connections(app_id, user_id).await {
            warn!("Cluster termination incomplete: {e}");
        }
    }

    async fn try_terminate_user_connections(
        &self,
        app_id: &str,
        user_id: &str,
    ) -> anyhow::Result<()> {
        self.local.terminate_user_connections(app_id, user_id).await;
        anyhow::ensure!(self.is_ready(), "Redis adapter is recovering");
        let batch = self
            .pubsub
            .request(
                &self.node_id,
                RequestType::TerminateUser {
                    app_id: app_id.to_owned(),
                    user_id: user_id.to_owned(),
                },
            )
            .await?;
        anyhow::ensure!(
            batch.complete
                && batch
                    .responses
                    .iter()
                    .all(|r| matches!(r, ResponsePayload::Ack)),
            "missing peer acknowledgement"
        );
        Ok(())
    }

    async fn get_socket(&self, app_id: &str, socket_id: &str) -> Option<Arc<WsConnection>> {
        self.local.get_socket(app_id, socket_id).await
    }

    async fn get_local_connection_count(&self, app_id: &str) -> usize {
        self.local.get_connection_count(app_id).await
    }

    async fn get_local_channels(&self, app_id: &str) -> Vec<String> {
        self.local.get_channels(app_id).await
    }

    async fn add_user(&self, app_id: &str, user_id: &str, socket_id: &str) {
        self.local.add_user(app_id, user_id, socket_id).await;
        if !self.pubsub.is_subscribed()
            && let Some(socket) = self.local.get_socket(app_id, socket_id).await
        {
            socket.force_close();
        }
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
        let mut conn = self.cmd_conn.clone();
        // Every first-local join changes the token so an older vacate check
        // cannot delete a newer occupancy claim, including on the same node.
        match redis::cmd("EVAL")
            .arg("local old = redis.call('EXISTS', KEYS[1]); redis.call('SET', KEYS[1], ARGV[1], 'EX', ARGV[2]); return 1-old")
            .arg(1).arg(&key).arg(uuid::Uuid::new_v4().to_string()).arg(OCCUPANCY_TTL_SECS)
            .query_async(&mut conn).await {
            Ok(first) => first,
            Err(e) => { warn!("Redis occupancy claim failed: {e}"); false }
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
        let key = self.occupancy_key(app_id, channel);
        let mut conn = self.cmd_conn.clone();
        let token = match conn.get::<_, Option<String>>(&key).await {
            Ok(Some(token)) => token,
            Ok(None) => return false,
            Err(e) => {
                warn!("Redis occupancy read failed: {e}");
                return false;
            }
        };
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
        let complete_zero = match self.pubsub.request(&self.node_id, request).await {
            Ok(batch) => {
                let remote: usize = batch
                    .responses
                    .iter()
                    .filter_map(|r| match r {
                        ResponsePayload::Count { count } => Some(*count),
                        _ => None,
                    })
                    .sum();
                if batch.complete
                    && vacated_from_peer_rpc(batch.expected, batch.responses.len(), remote)
                {
                    true
                } else {
                    if !batch.complete || batch.responses.len() != batch.expected {
                        self.pubsub
                            .metrics
                            .vacate_rpc_incomplete
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        warn!(
                            expected = batch.expected,
                            received = batch.responses.len(),
                            remote,
                            "Vacate RPC incomplete; not deleting occupancy"
                        );
                    }
                    false
                }
            }
            Err(e) => {
                self.pubsub
                    .metrics
                    .vacate_rpc_incomplete
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                warn!("Vacate RPC failed: {e}");
                false
            }
        };
        if !complete_zero {
            return false;
        }
        redis::cmd("EVAL")
            .arg("if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end; return 0")
            .arg(1).arg(&key).arg(token)
            .query_async(&mut conn).await.unwrap_or_else(|e| { warn!("Redis occupancy release failed: {e}"); false })
    }

    fn is_ready(&self) -> bool {
        self.pubsub.is_subscribed() && self.commands_ready.load(Ordering::Acquire)
    }

    async fn local_sockets(&self) -> Vec<Arc<WsConnection>> {
        self.local.local_sockets().await
    }
}

#[cfg(test)]
mod occupancy_tests {
    use super::vacated_from_peer_rpc;

    #[test]
    fn expected_zero_is_vacated() {
        assert!(vacated_from_peer_rpc(0, 0, 0));
    }

    #[test]
    fn complete_zero_is_vacated() {
        assert!(vacated_from_peer_rpc(2, 2, 0));
    }

    #[test]
    fn timeout_partial_is_not_vacated() {
        assert!(!vacated_from_peer_rpc(3, 1, 0));
        assert!(!vacated_from_peer_rpc(2, 0, 0));
    }

    #[test]
    fn complete_but_remote_members_is_not_vacated() {
        assert!(!vacated_from_peer_rpc(2, 2, 5));
    }

    #[test]
    fn oneshot_cancel_empty_with_peers_is_not_vacated() {
        assert!(!vacated_from_peer_rpc(1, 0, 0));
    }
}

#[cfg(test)]
mod live_tests {
    /// Optional live Redis check. Compiles in every `cargo test`; no-ops without REDIS_URL.
    /// CI: export REDIS_URL=redis://127.0.0.1:6379
    #[tokio::test]
    async fn quota_lua_set_get_and_reap() {
        let Ok(url) = std::env::var("REDIS_URL") else {
            eprintln!("skipping live Redis test (set REDIS_URL to run)");
            return;
        };
        let client = redis::Client::open(url.as_str()).expect("redis url");
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .expect("redis connect");
        let hash = format!("rushsocket-test:connections:{}", uuid::Uuid::new_v4());
        let node = "node-a";
        let stats_prefix = "rushsocket-test:node_stats:";
        let stats_key = format!("{stats_prefix}{node}");
        let _: () = redis::cmd("SET")
            .arg(&stats_key)
            .arg("{}")
            .arg("EX")
            .arg(15)
            .query_async(&mut conn)
            .await
            .unwrap();
        let set: Vec<i64> = redis::cmd("EVAL")
            .arg(super::lua::QUOTA_LUA)
            .arg(1)
            .arg(&hash)
            .arg(node)
            .arg(2)
            .arg(10)
            .arg(1)
            .arg(stats_prefix)
            .query_async(&mut conn)
            .await
            .unwrap();
        assert_eq!(set[0], 1);
        assert_eq!(set[1], 2);
        let get: Vec<i64> = redis::cmd("EVAL")
            .arg(super::lua::QUOTA_LUA)
            .arg(1)
            .arg(&hash)
            .arg(node)
            .arg(0)
            .arg(-1)
            .arg(0)
            .arg(stats_prefix)
            .query_async(&mut conn)
            .await
            .unwrap();
        assert_eq!(get[1], 2);
        let _: () = redis::cmd("DEL")
            .arg(&hash)
            .arg(&stats_key)
            .query_async(&mut conn)
            .await
            .unwrap();
    }
}
