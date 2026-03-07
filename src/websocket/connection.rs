use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::app::App;
use crate::protocol::messages::ServerMessage;

pub struct WsConnection {
    pub socket_id: Arc<str>,
    pub app: Arc<App>,
    pub tx: mpsc::Sender<Bytes>,
    pub subscribed_channels: Mutex<HashSet<String>>,
    /// For presence channels: channel -> (user_id, user_info)
    pub presence: Mutex<HashMap<String, (String, serde_json::Value)>>,
    /// Authenticated user info (from pusher:signin)
    pub user: Mutex<Option<UserInfo>>,
    /// Whether the user has been authenticated (for user auth timeout)
    pub user_authenticated: AtomicBool,
    /// Whether a server ping has been sent and we're waiting for pong
    pinged: AtomicBool,
    /// Set when the outbound channel is full (slow consumer); connection will be closed.
    force_close: AtomicBool,
}

#[derive(Debug, Clone)]
pub struct UserInfo {
    pub id: String,
    pub user_data: serde_json::Value,
}

impl WsConnection {
    pub fn new(socket_id: Arc<str>, app: Arc<App>, tx: mpsc::Sender<Bytes>) -> Self {
        Self {
            socket_id,
            app,
            tx,
            subscribed_channels: Mutex::new(HashSet::new()),
            presence: Mutex::new(HashMap::new()),
            user: Mutex::new(None),
            user_authenticated: AtomicBool::new(false),
            pinged: AtomicBool::new(false),
            force_close: AtomicBool::new(false),
        }
    }

    pub fn send(&self, msg: &ServerMessage) -> bool {
        self.send_raw(Bytes::from(msg.to_json()))
    }

    pub fn send_raw(&self, json: Bytes) -> bool {
        if self.tx.try_send(json).is_ok() {
            true
        } else {
            self.force_close.store(true, Ordering::Relaxed);
            false
        }
    }

    /// Whether the connection should be forcefully closed (slow consumer or terminated).
    pub fn should_close(&self) -> bool {
        self.force_close.load(Ordering::Relaxed)
    }

    /// Mark this connection for forced closure (used by terminate API).
    pub fn force_close(&self) {
        self.force_close.store(true, Ordering::Relaxed);
    }

    /// Mark that we sent a server ping and are waiting for pong.
    pub fn mark_pinged(&self) {
        self.pinged.store(true, Ordering::Relaxed);
    }

    /// Clear the pinged flag (received activity/pong).
    pub fn clear_pinged(&self) {
        self.pinged.store(false, Ordering::Relaxed);
    }

    /// Connection is stale if it was pinged but never responded.
    pub fn is_stale(&self) -> bool {
        self.pinged.load(Ordering::Relaxed)
    }

    pub fn add_channel(&self, channel: String) {
        self.subscribed_channels.lock().insert(channel);
    }

    pub fn remove_channel(&self, channel: &str) {
        self.subscribed_channels.lock().remove(channel);
    }

    pub fn is_subscribed(&self, channel: &str) -> bool {
        self.subscribed_channels.lock().contains(channel)
    }

    pub fn get_channels(&self) -> HashSet<String> {
        self.subscribed_channels.lock().clone()
    }

    pub fn set_presence(
        &self,
        channel: String,
        user_id: String,
        user_info: serde_json::Value,
    ) {
        self.presence.lock().insert(channel, (user_id, user_info));
    }

    pub fn remove_presence(&self, channel: &str) -> Option<(String, serde_json::Value)> {
        self.presence.lock().remove(channel)
    }

    pub fn get_presence(&self, channel: &str) -> Option<(String, serde_json::Value)> {
        self.presence.lock().get(channel).cloned()
    }
}
