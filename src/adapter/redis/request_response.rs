use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Request types for cross-node queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RequestType {
    GetConnectionCount { app_id: String },
    GetChannels { app_id: String },
    GetChannelSocketCount { app_id: String, channel: String },
    GetChannelInfo { app_id: String, channel: String },
    GetPresenceMembers { app_id: String, channel: String },
    GetPresenceUserCount { app_id: String, channel: String },
    GetChannelSockets { app_id: String, channel: String },
    GetMetrics,
    /// Lightweight ping used for peer discovery on startup/reconnect.
    DiscoverPeers,
}

/// A request envelope with routing info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub request_id: String,
    pub sender_node: String,
    pub request_type: RequestType,
}

/// Response payload variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsePayload {
    Count(usize),
    Channels(Vec<String>),
    ChannelInfo {
        occupied: bool,
        subscription_count: usize,
        user_count: Option<usize>,
    },
    PresenceMembers(HashMap<String, serde_json::Value>),
    SocketIds(Vec<String>),
    Metrics(serde_json::Value),
    /// Acknowledgement for DiscoverPeers requests.
    Ack,
}

/// A response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub request_id: String,
    pub responder_node: String,
    pub payload: ResponsePayload,
}
