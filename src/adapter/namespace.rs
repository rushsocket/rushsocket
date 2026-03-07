use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use dashmap::DashMap;
use tracing::warn;

use crate::websocket::connection::WsConnection;

/// A namespace holds per-app state: all sockets, channel memberships, and presence data.
pub struct Namespace {
    /// socket_id -> connection
    pub sockets: DashMap<Arc<str>, Arc<WsConnection>>,
    /// Atomic connection counter for race-free admission control.
    connection_count: AtomicUsize,
    /// channel -> set of socket_ids
    pub channels: DashMap<String, HashSet<Arc<str>>>,
    /// socket_id -> set of channels (reverse index for O(1) disconnect cleanup)
    socket_channels: DashMap<Arc<str>, HashSet<String>>,
    /// channel -> (user_id -> (user_info, set of socket_ids for this user))
    pub presence: DashMap<String, HashMap<String, (serde_json::Value, HashSet<String>)>>,
    /// user_id -> set of socket_ids (for user-level operations)
    pub users: DashMap<String, HashSet<String>>,
}

impl Namespace {
    pub fn new() -> Self {
        Self {
            sockets: DashMap::new(),
            connection_count: AtomicUsize::new(0),
            channels: DashMap::new(),
            socket_channels: DashMap::new(),
            presence: DashMap::new(),
            users: DashMap::new(),
        }
    }

    pub fn add_socket(&self, conn: Arc<WsConnection>) {
        let sid = conn.socket_id.clone();
        self.sockets.insert(sid, conn);
        self.connection_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Atomically reserve a connection slot and insert the socket.
    /// Returns false (and does not insert) if the limit would be exceeded.
    /// A negative limit means unlimited.
    pub fn try_add_socket(&self, conn: Arc<WsConnection>, limit: i64) -> bool {
        if limit < 0 {
            self.add_socket(conn);
            return true;
        }
        let max = limit as usize;
        // CAS loop: atomically reserve a slot
        loop {
            let current = self.connection_count.load(Ordering::Relaxed);
            if current >= max {
                return false;
            }
            if self.connection_count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ).is_ok() {
                let sid = conn.socket_id.clone();
                self.sockets.insert(sid, conn);
                return true;
            }
        }
    }

    pub fn remove_socket(&self, socket_id: &str) -> Vec<String> {
        if self.sockets.remove(socket_id).is_some() {
            self.connection_count.fetch_sub(1, Ordering::Relaxed);
        }
        let mut vacated = Vec::new();

        // Use reverse index to find only the channels this socket was in
        if let Some((_, channels)) = self.socket_channels.remove(socket_id) {
            for ch in channels {
                let mut is_empty = false;
                if let Some(mut sockets) = self.channels.get_mut(&ch) {
                    sockets.remove(socket_id);
                    is_empty = sockets.is_empty();
                }
                if is_empty {
                    self.channels.remove(&ch);
                    vacated.push(ch);
                }
            }
        }
        vacated
    }

    /// Add a socket to a channel. Returns true if this is the first subscriber (channel occupied).
    pub fn add_to_channel(&self, socket_id: &str, channel: &str) -> bool {
        // Look up the Arc<str> from sockets to share the allocation
        let sid_arc = match self.sockets.get(socket_id) {
            Some(conn) => conn.socket_id.clone(),
            None => Arc::from(socket_id),
        };
        let mut entry = self.channels.entry(channel.to_string()).or_default();
        let was_empty = entry.is_empty();
        entry.insert(sid_arc.clone());
        // Maintain reverse index
        self.socket_channels
            .entry(sid_arc)
            .or_default()
            .insert(channel.to_string());
        was_empty
    }

    pub fn remove_from_channel(&self, socket_id: &str, channel: &str) -> bool {
        let mut vacated = false;
        if let Some(mut sockets) = self.channels.get_mut(channel) {
            sockets.remove(socket_id);
            if sockets.is_empty() {
                vacated = true;
            }
        }
        if vacated {
            self.channels.remove(channel);
        }
        // Update reverse index
        if let Some(mut chans) = self.socket_channels.get_mut(socket_id) {
            chans.remove(channel);
        }
        vacated
    }

    pub fn send_to_channel(&self, channel: &str, message: Bytes, excepting_id: Option<&str>) {
        if let Some(socket_ids) = self.channels.get(channel) {
            for sid in socket_ids.iter() {
                if Some(sid.as_ref()) == excepting_id {
                    continue;
                }
                if let Some(conn) = self.sockets.get(sid.as_ref()) {
                    if !conn.send_raw(message.clone()) {
                        warn!(
                            socket_id = sid.as_ref(),
                            channel,
                            "Slow consumer: outbound buffer full, connection will be closed"
                        );
                    }
                }
            }
        }
    }

    pub fn get_connection_count(&self) -> usize {
        self.connection_count.load(Ordering::Relaxed)
    }

    pub fn get_channels(&self) -> Vec<String> {
        self.channels.iter().map(|e| e.key().clone()).collect()
    }

    pub fn get_channel_socket_count(&self, channel: &str) -> usize {
        self.channels
            .get(channel)
            .map(|s| s.len())
            .unwrap_or(0)
    }

    pub fn get_presence_members(
        &self,
        channel: &str,
    ) -> HashMap<String, serde_json::Value> {
        self.presence
            .get(channel)
            .map(|members| {
                members
                    .iter()
                    .map(|(uid, (info, _))| (uid.clone(), info.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn add_presence_member(
        &self,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: &serde_json::Value,
    ) -> bool {
        let mut channel_presence = self
            .presence
            .entry(channel.to_string())
            .or_default();
        let is_new = !channel_presence.contains_key(user_id);
        channel_presence
            .entry(user_id.to_string())
            .and_modify(|(_, sockets)| {
                sockets.insert(socket_id.to_string());
            })
            .or_insert_with(|| {
                let mut sockets = HashSet::new();
                sockets.insert(socket_id.to_string());
                (user_info.clone(), sockets)
            });
        is_new
    }

    /// Atomically check the presence member limit, add a member, and return the current members.
    /// Returns `Ok((is_new, members))` on success, `Err(())` if over limit.
    /// Existing users are always allowed (additional sockets for the same user).
    /// The members snapshot is taken while still holding the entry guard, avoiding a second lookup.
    pub fn try_add_presence_member(
        &self,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: &serde_json::Value,
        max_members: usize,
    ) -> Result<(bool, HashMap<String, serde_json::Value>), ()> {
        let mut channel_presence = self
            .presence
            .entry(channel.to_string())
            .or_default();
        let already_present = channel_presence.contains_key(user_id);
        if !already_present && channel_presence.len() >= max_members {
            return Err(());
        }
        channel_presence
            .entry(user_id.to_string())
            .and_modify(|(_, sockets)| {
                sockets.insert(socket_id.to_string());
            })
            .or_insert_with(|| {
                let mut sockets = HashSet::new();
                sockets.insert(socket_id.to_string());
                (user_info.clone(), sockets)
            });
        // Snapshot members while we still hold the guard
        let members: HashMap<String, serde_json::Value> = channel_presence
            .iter()
            .map(|(uid, (info, _))| (uid.clone(), info.clone()))
            .collect();
        Ok((!already_present, members))
    }

    pub fn remove_presence_member(
        &self,
        channel: &str,
        socket_id: &str,
        user_id: &str,
    ) -> bool {
        let mut user_gone = false;
        if let Some(mut channel_presence) = self.presence.get_mut(channel) {
            if let Some((_, sockets)) = channel_presence.get_mut(user_id) {
                sockets.remove(socket_id);
                if sockets.is_empty() {
                    channel_presence.remove(user_id);
                    user_gone = true;
                }
            }
            if channel_presence.is_empty() {
                drop(channel_presence);
                self.presence.remove(channel);
            }
        }
        user_gone
    }

    pub fn get_presence_user_count(&self, channel: &str) -> usize {
        self.presence
            .get(channel)
            .map(|m| m.len())
            .unwrap_or(0)
    }

    pub fn add_user(&self, user_id: &str, socket_id: &str) {
        self.users
            .entry(user_id.to_string())
            .or_default()
            .insert(socket_id.to_string());
    }

    pub fn remove_user(&self, user_id: &str, socket_id: &str) {
        if let Some(mut sockets) = self.users.get_mut(user_id) {
            sockets.remove(socket_id);
            if sockets.is_empty() {
                drop(sockets);
                self.users.remove(user_id);
            }
        }
    }

    pub fn get_sockets_for_user(&self, user_id: &str) -> Vec<Arc<WsConnection>> {
        self.users
            .get(user_id)
            .map(|socket_ids| {
                socket_ids
                    .iter()
                    .filter_map(|sid| self.sockets.get(sid.as_str()).map(|c| c.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use tokio::sync::mpsc;

    fn make_conn(socket_id: &str) -> Arc<WsConnection> {
        let (tx, _rx) = mpsc::channel(512);
        let app = Arc::new(App {
            id: "test-app".to_string(),
            key: "test-key".to_string(),
            secret: "test-secret".to_string(),
            enabled: true,
            client_event_mode: "all".to_string(),
            enable_user_authentication: true,
            max_connections: -1,
            max_backend_events_per_sec: -1,
            max_client_events_per_sec: -1,
            max_read_req_per_sec: -1,
            max_presence_members_per_channel: 200,
            max_presence_member_size_in_kb: 2,
            max_channel_name_length: 256,
            max_event_channels_at_once: 200,
            max_event_name_length: 256,
            max_event_payload_in_kb: 32,
            max_event_batch_size: 20,
            enable_cache_channels: true,
            allowed_origins: vec![],
            max_message_size_in_kb: 40,
            webhook_url: None,
            webhook_batch_ms: 1000,
            enable_subscription_count_webhook: false,
        });
        Arc::new(WsConnection::new(Arc::from(socket_id), app, tx))
    }

    #[test]
    fn test_add_and_remove_socket() {
        let ns = Namespace::new();
        let conn = make_conn("1.1");
        ns.add_socket(conn);
        assert_eq!(ns.get_connection_count(), 1);

        ns.remove_socket("1.1");
        assert_eq!(ns.get_connection_count(), 0);
    }

    #[test]
    fn test_channel_join_leave() {
        let ns = Namespace::new();
        let occupied = ns.add_to_channel("1.1", "my-channel");
        assert!(occupied); // first subscriber
        assert_eq!(ns.get_channel_socket_count("my-channel"), 1);
        assert_eq!(ns.get_channels().len(), 1);

        let vacated = ns.remove_from_channel("1.1", "my-channel");
        assert!(vacated);
        assert_eq!(ns.get_channel_socket_count("my-channel"), 0);
        assert!(ns.get_channels().is_empty());
    }

    #[test]
    fn test_channel_not_vacated_with_remaining() {
        let ns = Namespace::new();
        let occupied1 = ns.add_to_channel("1.1", "ch");
        let occupied2 = ns.add_to_channel("2.2", "ch");
        assert!(occupied1);  // first subscriber
        assert!(!occupied2); // second subscriber, not newly occupied
        assert_eq!(ns.get_channel_socket_count("ch"), 2);

        let vacated = ns.remove_from_channel("1.1", "ch");
        assert!(!vacated);
        assert_eq!(ns.get_channel_socket_count("ch"), 1);
    }

    #[test]
    fn test_remove_socket_cleans_channels() {
        let ns = Namespace::new();
        let conn = make_conn("1.1");
        ns.add_socket(conn);
        ns.add_to_channel("1.1", "ch-a");
        ns.add_to_channel("1.1", "ch-b");

        let vacated = ns.remove_socket("1.1");
        assert_eq!(vacated.len(), 2);
        assert!(ns.get_channels().is_empty());
    }

    #[test]
    fn test_presence_add_new_member() {
        let ns = Namespace::new();
        let info = serde_json::json!({"name": "Alice"});
        let is_new = ns.add_presence_member("presence-room", "sock1", "user1", &info);
        assert!(is_new);
        assert_eq!(ns.get_presence_user_count("presence-room"), 1);

        let members = ns.get_presence_members("presence-room");
        assert_eq!(members.len(), 1);
        assert_eq!(members["user1"]["name"], "Alice");
    }

    #[test]
    fn test_presence_dedup_same_user() {
        let ns = Namespace::new();
        let info = serde_json::json!({"name": "Alice"});
        let is_new1 = ns.add_presence_member("presence-room", "sock1", "user1", &info);
        let is_new2 = ns.add_presence_member("presence-room", "sock2", "user1", &info);
        assert!(is_new1);
        assert!(!is_new2); // Same user, should NOT be new
        assert_eq!(ns.get_presence_user_count("presence-room"), 1);
    }

    #[test]
    fn test_presence_remove_last_socket_removes_user() {
        let ns = Namespace::new();
        let info = serde_json::json!({});
        ns.add_presence_member("ch", "sock1", "user1", &info);
        ns.add_presence_member("ch", "sock2", "user1", &info);

        let gone1 = ns.remove_presence_member("ch", "sock1", "user1");
        assert!(!gone1); // user1 still has sock2

        let gone2 = ns.remove_presence_member("ch", "sock2", "user1");
        assert!(gone2); // user1 has no more sockets
        assert_eq!(ns.get_presence_user_count("ch"), 0);
    }

    #[test]
    fn test_user_tracking() {
        let ns = Namespace::new();
        let conn = make_conn("sock1");
        ns.add_socket(conn);
        ns.add_user("user1", "sock1");

        let sockets = ns.get_sockets_for_user("user1");
        assert_eq!(sockets.len(), 1);
        assert_eq!(&*sockets[0].socket_id, "sock1");

        ns.remove_user("user1", "sock1");
        let sockets = ns.get_sockets_for_user("user1");
        assert!(sockets.is_empty());
    }
}
