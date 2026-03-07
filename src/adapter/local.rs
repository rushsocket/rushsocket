use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;

use super::namespace::Namespace;
use super::{Adapter, ChannelInfo};
use crate::protocol::channels::ChannelKind;
use crate::websocket::connection::WsConnection;

pub struct LocalAdapter {
    namespaces: DashMap<String, Arc<Namespace>>,
}

impl LocalAdapter {
    pub fn new() -> Self {
        Self {
            namespaces: DashMap::new(),
        }
    }

    fn get_or_create_ns(&self, app_id: &str) -> Arc<Namespace> {
        self.namespaces
            .entry(app_id.to_string())
            .or_insert_with(|| Arc::new(Namespace::new()))
            .clone()
    }

    fn get_ns(&self, app_id: &str) -> Option<Arc<Namespace>> {
        self.namespaces.get(app_id).map(|ns| ns.clone())
    }

    /// Get all app IDs that have active namespaces.
    pub fn get_app_ids(&self) -> Vec<String> {
        self.namespaces.iter().map(|e| e.key().clone()).collect()
    }
}

#[async_trait]
impl Adapter for LocalAdapter {
    async fn add_socket(&self, app_id: &str, conn: Arc<WsConnection>) {
        self.get_or_create_ns(app_id).add_socket(conn);
    }

    async fn try_add_socket(&self, app_id: &str, conn: Arc<WsConnection>, limit: i64) -> bool {
        self.get_or_create_ns(app_id).try_add_socket(conn, limit)
    }

    async fn remove_socket(&self, app_id: &str, socket_id: &str) -> Vec<String> {
        if let Some(ns) = self.get_ns(app_id) {
            ns.remove_socket(socket_id)
        } else {
            vec![]
        }
    }

    async fn add_to_channel(&self, app_id: &str, socket_id: &str, channel: &str) -> bool {
        self.get_or_create_ns(app_id)
            .add_to_channel(socket_id, channel)
    }

    async fn remove_from_channel(&self, app_id: &str, socket_id: &str, channel: &str) -> bool {
        if let Some(ns) = self.get_ns(app_id) {
            ns.remove_from_channel(socket_id, channel)
        } else {
            false
        }
    }

    async fn send_to_channel(
        &self,
        app_id: &str,
        channel: &str,
        message: Bytes,
        excepting_id: Option<&str>,
    ) {
        if let Some(ns) = self.get_ns(app_id) {
            ns.send_to_channel(channel, message, excepting_id);
        }
    }

    async fn get_connection_count(&self, app_id: &str) -> usize {
        self.get_ns(app_id)
            .map(|ns| ns.get_connection_count())
            .unwrap_or(0)
    }

    async fn get_channels(&self, app_id: &str) -> Vec<String> {
        self.get_ns(app_id)
            .map(|ns| ns.get_channels())
            .unwrap_or_default()
    }

    async fn get_channel_info(&self, app_id: &str, channel: &str) -> ChannelInfo {
        let ns = match self.get_ns(app_id) {
            Some(ns) => ns,
            None => return ChannelInfo::default(),
        };
        let count = ns.get_channel_socket_count(channel);
        let kind = ChannelKind::from_name(channel);
        let user_count = if kind.is_presence() {
            Some(ns.get_presence_user_count(channel))
        } else {
            None
        };
        ChannelInfo {
            occupied: count > 0,
            subscription_count: count,
            user_count,
        }
    }

    async fn get_channel_socket_count(&self, app_id: &str, channel: &str) -> usize {
        self.get_ns(app_id)
            .map(|ns| ns.get_channel_socket_count(channel))
            .unwrap_or(0)
    }

    async fn get_presence_members(
        &self,
        app_id: &str,
        channel: &str,
    ) -> HashMap<String, serde_json::Value> {
        self.get_ns(app_id)
            .map(|ns| ns.get_presence_members(channel))
            .unwrap_or_default()
    }

    async fn add_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: &serde_json::Value,
    ) -> bool {
        self.get_or_create_ns(app_id)
            .add_presence_member(channel, socket_id, user_id, user_info)
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
        self.get_or_create_ns(app_id)
            .try_add_presence_member(channel, socket_id, user_id, user_info, max_members)
    }

    async fn remove_presence_member(
        &self,
        app_id: &str,
        channel: &str,
        socket_id: &str,
        user_id: &str,
    ) -> bool {
        if let Some(ns) = self.get_ns(app_id) {
            ns.remove_presence_member(channel, socket_id, user_id)
        } else {
            false
        }
    }

    async fn get_presence_user_count(&self, app_id: &str, channel: &str) -> usize {
        self.get_ns(app_id)
            .map(|ns| ns.get_presence_user_count(channel))
            .unwrap_or(0)
    }

    async fn get_sockets_for_user(
        &self,
        app_id: &str,
        user_id: &str,
    ) -> Vec<Arc<WsConnection>> {
        self.get_ns(app_id)
            .map(|ns| ns.get_sockets_for_user(user_id))
            .unwrap_or_default()
    }

    async fn terminate_user_connections(&self, app_id: &str, user_id: &str) {
        if let Some(ns) = self.get_ns(app_id) {
            let conns = ns.get_sockets_for_user(user_id);
            for conn in conns {
                // Best-effort: try to send an error frame before closing
                let msg = bytes::Bytes::from(
                    crate::protocol::messages::ServerMessage::error(
                        "Connection terminated by server",
                        Some(4009),
                    )
                    .to_json(),
                );
                let _ = conn.tx.try_send(msg);
                // Force the connection loop to break regardless of whether the
                // error frame was enqueued (buffer may be full).
                conn.force_close();
            }
        }
    }

    async fn get_socket(
        &self,
        app_id: &str,
        socket_id: &str,
    ) -> Option<Arc<WsConnection>> {
        self.get_ns(app_id)
            .and_then(|ns| ns.sockets.get(socket_id).map(|c| c.clone()))
    }

    async fn get_channel_sockets(&self, app_id: &str, channel: &str) -> Vec<String> {
        self.get_ns(app_id)
            .and_then(|ns| {
                ns.channels
                    .get(channel)
                    .map(|s| s.iter().map(|sid| sid.to_string()).collect())
            })
            .unwrap_or_default()
    }

    async fn add_user(&self, app_id: &str, user_id: &str, socket_id: &str) {
        self.get_or_create_ns(app_id).add_user(user_id, socket_id);
    }

    async fn remove_user(&self, app_id: &str, user_id: &str, socket_id: &str) {
        if let Some(ns) = self.get_ns(app_id) {
            ns.remove_user(user_id, socket_id);
        }
    }
}
