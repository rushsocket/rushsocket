pub mod local;
pub mod namespace;
pub mod redis;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;

use crate::websocket::connection::WsConnection;

/// Result of channel info queries.
#[derive(Debug, Clone, Default)]
pub struct ChannelInfo {
    pub occupied: bool,
    pub subscription_count: usize,
    pub user_count: Option<usize>,
}

/// The adapter trait abstracts single-node and multi-node storage.
#[async_trait]
pub trait Adapter: Send + Sync + 'static {
    /// Add a socket to an app namespace.
    async fn add_socket(&self, app_id: &str, conn: Arc<WsConnection>);

    /// Atomically check the connection limit and add a socket.
    /// Returns false (without inserting) if the limit would be exceeded.
    /// A negative limit means unlimited.
    async fn try_add_socket(&self, app_id: &str, conn: Arc<WsConnection>, _limit: i64) -> bool {
        self.add_socket(app_id, conn).await;
        true
    }

    /// Remove a socket from an app namespace. Returns list of channels it was in.
    async fn remove_socket(&self, app_id: &str, socket_id: &str) -> Vec<String>;

    /// Add a socket to a channel. Returns true if channel was just created (occupied).
    async fn add_to_channel(&self, app_id: &str, socket_id: &str, channel: &str) -> bool;

    /// Remove a socket from a channel. Returns true if channel is now empty (vacated).
    async fn remove_from_channel(&self, app_id: &str, socket_id: &str, channel: &str) -> bool;

    /// Send a pre-serialized message to all sockets in a channel, optionally excluding one socket.
    /// Takes `Bytes` so the caller serializes once and recipients share the refcount.
    async fn send_to_channel(
        &self,
        app_id: &str,
        channel: &str,
        message: Bytes,
        excepting_id: Option<&str>,
    );

    /// Get the connection count for an app.
    async fn get_connection_count(&self, app_id: &str) -> usize;

    /// Get all channel names for an app.
    async fn get_channels(&self, app_id: &str) -> Vec<String>;

    /// Get info about a specific channel.
    async fn get_channel_info(&self, app_id: &str, channel: &str) -> ChannelInfo;

    /// Get the number of sockets in a channel.
    async fn get_channel_socket_count(&self, app_id: &str, channel: &str) -> usize;

    /// Get presence members for a channel: user_id -> user_info.
    async fn get_presence_members(
        &self,
        app_id: &str,
        channel: &str,
    ) -> HashMap<String, serde_json::Value>;

    /// Add a presence member. Returns true if user was newly added (not already present).
    async fn add_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: &serde_json::Value,
    ) -> bool;

    /// Atomically check presence member limit, add a member, and return the current members.
    /// Returns `Ok((is_new, members))` on success, `Err(())` if over limit.
    async fn try_add_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: &serde_json::Value,
        _max_members: usize,
    ) -> Result<(bool, HashMap<String, serde_json::Value>), ()> {
        // Default: no atomic check, just add and fetch members
        let is_new = self.add_presence_member(app_id, channel, socket_id, user_id, user_info).await;
        let members = self.get_presence_members(app_id, channel).await;
        Ok((is_new, members))
    }

    /// Remove a presence member. Returns true if user has no more sockets (should emit member_removed).
    async fn remove_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
    ) -> bool;

    /// Get presence user count for a channel.
    async fn get_presence_user_count(&self, app_id: &str, channel: &str) -> usize;

    /// Get all sockets for a user across all channels in an app.
    async fn get_sockets_for_user(&self, app_id: &str, user_id: &str) -> Vec<Arc<WsConnection>>;

    /// Terminate all connections for a user in an app.
    async fn terminate_user_connections(&self, app_id: &str, user_id: &str);

    /// Get a socket by its ID.
    async fn get_socket(&self, app_id: &str, socket_id: &str) -> Option<Arc<WsConnection>>;

    /// Get all socket IDs in a channel.
    async fn get_channel_sockets(&self, app_id: &str, channel: &str) -> Vec<String>;

    /// Get local-only connection count (no cluster fan-out).
    async fn get_local_connection_count(&self, app_id: &str) -> usize {
        self.get_connection_count(app_id).await
    }

    /// Get local-only channels (no cluster fan-out).
    async fn get_local_channels(&self, app_id: &str) -> Vec<String> {
        self.get_channels(app_id).await
    }

    /// Get local-only presence user count (no cluster fan-out).
    async fn get_local_presence_user_count(&self, app_id: &str, channel: &str) -> usize {
        self.get_presence_user_count(app_id, channel).await
    }

    /// Track a user->socket mapping (for terminate API).
    async fn add_user(&self, _app_id: &str, _user_id: &str, _socket_id: &str) {}

    /// Remove a user->socket mapping (for terminate API cleanup).
    async fn remove_user(&self, _app_id: &str, _user_id: &str, _socket_id: &str) {}

    /// Get aggregated metrics across all nodes. Default returns None (use local metrics).
    async fn get_aggregated_metrics(&self) -> Option<serde_json::Value> {
        None
    }

    /// Check if a channel became globally occupied (first subscriber across all nodes).
    /// In local mode, `locally_occupied` is already globally correct.
    /// In Redis cluster mode, uses SETNX for deduplication.
    async fn is_channel_globally_occupied(
        &self,
        _app_id: &str,
        _channel: &str,
        locally_occupied: bool,
    ) -> bool {
        locally_occupied
    }

    /// Check if a channel became globally vacated (no subscribers across all nodes).
    /// In local mode, `locally_vacated` is already globally correct.
    /// In Redis cluster mode, checks global socket count and cleans up the occupancy key.
    async fn is_channel_globally_vacated(
        &self,
        _app_id: &str,
        _channel: &str,
        locally_vacated: bool,
    ) -> bool {
        locally_vacated
    }

    /// Whether the adapter is fully ready to serve traffic.
    /// Local adapter is always ready; Redis adapter is ready after subscriptions
    /// are live and peer discovery is complete.
    fn is_ready(&self) -> bool {
        true
    }
}
