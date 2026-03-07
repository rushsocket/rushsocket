use std::sync::Arc;

use crate::protocol::channels::ChannelKind;
use crate::protocol::messages::{ClientMessage, ServerMessage};
use crate::state::AppState;
use crate::webhook::WebhookEvent;
use crate::websocket::connection::WsConnection;

pub async fn handle(state: &Arc<AppState>, conn: &Arc<WsConnection>, msg: &ClientMessage) {
    let is_members_only = match conn.app.client_event_mode.as_str() {
        "none" => {
            conn.send(&ServerMessage::error(
                "Client events are not enabled for this app",
                Some(4301),
            ));
            return;
        }
        "members" => true,
        "all" | _ => false,
    };

    // Must have a channel
    let channel = match &msg.channel {
        Some(c) => c.clone(),
        None => {
            conn.send(&ServerMessage::error("No channel provided", Some(4009)));
            return;
        }
    };

    // Must be subscribed to the channel
    if !conn.is_subscribed(&channel) {
        conn.send(&ServerMessage::error(
            "Not subscribed to channel",
            Some(4009),
        ));
        return;
    }

    // In "members" mode, only presence channel members can send client events
    if is_members_only {
        let kind = ChannelKind::from_name(&channel);
        if !kind.is_presence() || conn.get_presence(&channel).is_none() {
            conn.send(&ServerMessage::error(
                "Client events require presence channel membership",
                Some(4301),
            ));
            return;
        }
    }

    // Rate limit check
    if !state
        .rate_limiter
        .check_client_event(&conn.socket_id, conn.app.max_client_events_per_sec)
    {
        conn.send(&ServerMessage::error(
            "Rate limit exceeded",
            Some(4301),
        ));
        return;
    }

    // Event name must start with "client-"
    if !msg.event.starts_with("client-") {
        conn.send(&ServerMessage::error(
            "Client event must start with 'client-'",
            Some(4009),
        ));
        return;
    }

    // Check event name length
    if msg.event.len() > conn.app.max_event_name_length {
        conn.send(&ServerMessage::error(
            "Event name too long",
            Some(4009),
        ));
        return;
    }

    // Check payload size (estimate without allocating a full JSON string)
    if estimate_json_size(&msg.data) > conn.app.max_event_payload_in_kb * 1024 {
        conn.send(&ServerMessage::error(
            "Event payload too large",
            Some(4009),
        ));
        return;
    }

    // Get user_id for presence channels (after size check)
    let user_id = conn.get_presence(&channel).map(|(uid, _)| uid);

    // Webhook for client events (before send, captures even if no other subscribers)
    if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(&conn.app.id)) {
        wh.send(WebhookEvent::client_event(
            &channel,
            &msg.event,
            &msg.data,
            &conn.socket_id,
            user_id.as_deref(),
        ));
    }

    // Forward to channel excluding sender
    let forward = ServerMessage::channel_event(&msg.event, &channel, msg.data.clone(), user_id);
    state
        .adapter
        .send_to_channel(
            &conn.app.id,
            &channel,
            bytes::Bytes::from(forward.to_json()),
            Some(&conn.socket_id),
        )
        .await;
}

/// Estimate the serialized JSON size of a `serde_json::Value` without allocating.
fn estimate_json_size(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Null => 4,        // "null"
        serde_json::Value::Bool(true) => 4,  // "true"
        serde_json::Value::Bool(false) => 5, // "false"
        serde_json::Value::Number(n) => {
            // Estimate: digits + sign + decimal point
            let s = n.to_string();
            s.len()
        }
        serde_json::Value::String(s) => s.len() + 2, // quotes; under-counts escapes, safe for limit check
        serde_json::Value::Array(arr) => {
            // brackets + commas + elements
            let inner: usize = arr.iter().map(estimate_json_size).sum();
            let commas = if arr.len() > 1 { arr.len() - 1 } else { 0 };
            2 + inner + commas
        }
        serde_json::Value::Object(map) => {
            // braces + key:value pairs + commas
            let inner: usize = map
                .iter()
                .map(|(k, v)| k.len() + 2 + 1 + estimate_json_size(v)) // "key":value
                .sum();
            let commas = if map.len() > 1 { map.len() - 1 } else { 0 };
            2 + inner + commas
        }
    }
}
