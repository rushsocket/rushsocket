use serde::{Deserialize, Serialize};

/// Request types for cross-node queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RequestType {
    GetChannelSocketCount {
        app_id: String,
        channel: String,
    },
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
    Count {
        count: usize,
    },
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
