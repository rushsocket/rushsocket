use serde::{Deserialize, Serialize};

/// Incoming message from a WebSocket client.
#[derive(Debug, Deserialize)]
pub struct ClientMessage {
    pub event: String,
    #[serde(default)]
    pub data: serde_json::Value,
    #[serde(default)]
    pub channel: Option<String>,
}

/// Outgoing message to a WebSocket client.
#[derive(Debug, Serialize, Clone)]
pub struct ServerMessage {
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

pub const PONG_JSON: &str = r#"{"event":"pusher:pong"}"#;
pub const PING_JSON: &str = r#"{"event":"pusher:ping"}"#;

impl ServerMessage {
    /// Connection established response (data is double-serialized JSON string).
    pub fn connection_established(socket_id: &str, activity_timeout: u32) -> Self {
        let inner = serde_json::json!({
            "socket_id": socket_id,
            "activity_timeout": activity_timeout,
        });
        ServerMessage {
            event: "pusher:connection_established".to_string(),
            data: Some(serde_json::Value::String(inner.to_string())),
            channel: None,
            user_id: None,
        }
    }

    pub fn pong() -> Self {
        ServerMessage {
            event: "pusher:pong".to_string(),
            data: None,
            channel: None,
            user_id: None,
        }
    }

    pub fn ping() -> Self {
        ServerMessage {
            event: "pusher:ping".to_string(),
            data: None,
            channel: None,
            user_id: None,
        }
    }

    pub fn error(message: &str, code: Option<u16>) -> Self {
        let mut data = serde_json::json!({ "message": message });
        if let Some(c) = code {
            data["code"] = serde_json::json!(c);
        }
        ServerMessage {
            event: "pusher:error".to_string(),
            data: Some(serde_json::Value::String(data.to_string())),
            channel: None,
            user_id: None,
        }
    }

    pub fn subscription_succeeded(channel: &str) -> Self {
        ServerMessage {
            event: "pusher_internal:subscription_succeeded".to_string(),
            data: Some(serde_json::Value::String("{}".to_string())),
            channel: Some(channel.to_string()),
            user_id: None,
        }
    }

    pub fn presence_subscription_succeeded(
        channel: &str,
        ids: &[String],
        hash: &serde_json::Value,
        count: usize,
    ) -> Self {
        let inner = serde_json::json!({
            "presence": {
                "ids": ids,
                "hash": hash,
                "count": count,
            }
        });
        ServerMessage {
            event: "pusher_internal:subscription_succeeded".to_string(),
            data: Some(serde_json::Value::String(inner.to_string())),
            channel: Some(channel.to_string()),
            user_id: None,
        }
    }

    pub fn member_added(channel: &str, user_id: &str, user_info: &serde_json::Value) -> Self {
        let inner = serde_json::json!({
            "user_id": user_id,
            "user_info": user_info,
        });
        ServerMessage {
            event: "pusher_internal:member_added".to_string(),
            data: Some(serde_json::Value::String(inner.to_string())),
            channel: Some(channel.to_string()),
            user_id: None,
        }
    }

    pub fn member_removed(channel: &str, user_id: &str) -> Self {
        let inner = serde_json::json!({ "user_id": user_id });
        ServerMessage {
            event: "pusher_internal:member_removed".to_string(),
            data: Some(serde_json::Value::String(inner.to_string())),
            channel: Some(channel.to_string()),
            user_id: None,
        }
    }

    pub fn channel_event(
        event: &str,
        channel: &str,
        data: serde_json::Value,
        user_id: Option<String>,
    ) -> Self {
        ServerMessage {
            event: event.to_string(),
            data: Some(data),
            channel: Some(channel.to_string()),
            user_id,
        }
    }

    pub fn cache_miss(channel: &str) -> Self {
        ServerMessage {
            event: "pusher:cache_miss".to_string(),
            data: None,
            channel: Some(channel.to_string()),
            user_id: None,
        }
    }

    pub fn signin_success(user_data: &str) -> Self {
        let inner = serde_json::json!({ "user_data": user_data });
        ServerMessage {
            event: "pusher:signin_success".to_string(),
            data: Some(serde_json::Value::String(inner.to_string())),
            channel: None,
            user_id: None,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_established_double_serialized() {
        let msg = ServerMessage::connection_established("123.456", 30);
        assert_eq!(msg.event, "pusher:connection_established");

        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        // data should be a string (double-serialized)
        let data_str = json["data"].as_str().unwrap();
        // Parse the inner JSON
        let inner: serde_json::Value = serde_json::from_str(data_str).unwrap();
        assert_eq!(inner["socket_id"], "123.456");
        assert_eq!(inner["activity_timeout"], 30);
    }

    #[test]
    fn test_pong() {
        let msg = ServerMessage::pong();
        assert_eq!(msg.event, "pusher:pong");
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert!(json.get("data").is_none());
    }

    #[test]
    fn test_error_with_code() {
        let msg = ServerMessage::error("Something went wrong", Some(4001));
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(json["event"], "pusher:error");
        let data_str = json["data"].as_str().unwrap();
        let inner: serde_json::Value = serde_json::from_str(data_str).unwrap();
        assert_eq!(inner["message"], "Something went wrong");
        assert_eq!(inner["code"], 4001);
    }

    #[test]
    fn test_error_without_code() {
        let msg = ServerMessage::error("Oops", None);
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        let data_str = json["data"].as_str().unwrap();
        let inner: serde_json::Value = serde_json::from_str(data_str).unwrap();
        assert_eq!(inner["message"], "Oops");
        assert!(inner.get("code").is_none());
    }

    #[test]
    fn test_subscription_succeeded() {
        let msg = ServerMessage::subscription_succeeded("my-channel");
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(json["event"], "pusher_internal:subscription_succeeded");
        assert_eq!(json["channel"], "my-channel");
        assert_eq!(json["data"], "{}");
    }

    #[test]
    fn test_presence_subscription_succeeded_double_serialized() {
        let ids = vec!["user1".to_string(), "user2".to_string()];
        let hash = serde_json::json!({"user1": {"name": "Alice"}, "user2": {"name": "Bob"}});
        let msg = ServerMessage::presence_subscription_succeeded("presence-room", &ids, &hash, 2);
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(json["event"], "pusher_internal:subscription_succeeded");
        assert_eq!(json["channel"], "presence-room");
        // data is a string (double-serialized)
        let data_str = json["data"].as_str().unwrap();
        let inner: serde_json::Value = serde_json::from_str(data_str).unwrap();
        assert_eq!(inner["presence"]["count"], 2);
        assert!(inner["presence"]["ids"].is_array());
        assert!(inner["presence"]["hash"].is_object());
    }

    #[test]
    fn test_member_added() {
        let msg = ServerMessage::member_added("presence-room", "user1", &serde_json::json!({"name": "Alice"}));
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(json["event"], "pusher_internal:member_added");
        assert_eq!(json["channel"], "presence-room");
        let data_str = json["data"].as_str().unwrap();
        let inner: serde_json::Value = serde_json::from_str(data_str).unwrap();
        assert_eq!(inner["user_id"], "user1");
        assert_eq!(inner["user_info"]["name"], "Alice");
    }

    #[test]
    fn test_member_removed() {
        let msg = ServerMessage::member_removed("presence-room", "user1");
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(json["event"], "pusher_internal:member_removed");
        let data_str = json["data"].as_str().unwrap();
        let inner: serde_json::Value = serde_json::from_str(data_str).unwrap();
        assert_eq!(inner["user_id"], "user1");
    }

    #[test]
    fn test_channel_event() {
        let msg = ServerMessage::channel_event(
            "my-event",
            "my-channel",
            serde_json::json!("hello"),
            Some("user1".to_string()),
        );
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(json["event"], "my-event");
        assert_eq!(json["channel"], "my-channel");
        assert_eq!(json["data"], "hello");
        assert_eq!(json["user_id"], "user1");
    }

    #[test]
    fn test_channel_event_no_user_id_omitted() {
        let msg = ServerMessage::channel_event("ev", "ch", serde_json::json!("x"), None);
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert!(json.get("user_id").is_none());
    }

    #[test]
    fn test_cache_miss() {
        let msg = ServerMessage::cache_miss("cache-my-channel");
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(json["event"], "pusher:cache_miss");
        assert_eq!(json["channel"], "cache-my-channel");
    }

    #[test]
    fn test_signin_success() {
        let msg = ServerMessage::signin_success(r#"{"id":"user1"}"#);
        let json: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(json["event"], "pusher:signin_success");
        let data_str = json["data"].as_str().unwrap();
        let inner: serde_json::Value = serde_json::from_str(data_str).unwrap();
        assert_eq!(inner["user_data"], r#"{"id":"user1"}"#);
    }

    #[test]
    fn test_client_message_deserialize() {
        let raw = r#"{"event":"pusher:subscribe","data":{"channel":"my-ch"}}"#;
        let msg: ClientMessage = serde_json::from_str(raw).unwrap();
        assert_eq!(msg.event, "pusher:subscribe");
        assert_eq!(msg.data["channel"], "my-ch");
        assert!(msg.channel.is_none());
    }

    #[test]
    fn test_client_message_with_channel() {
        let raw = r#"{"event":"client-msg","data":"hello","channel":"private-chat"}"#;
        let msg: ClientMessage = serde_json::from_str(raw).unwrap();
        assert_eq!(msg.event, "client-msg");
        assert_eq!(msg.channel.as_deref(), Some("private-chat"));
    }
}
