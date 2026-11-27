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
        "all" => false,
        other => {
            tracing::warn!(mode = other, "unknown client_event_mode; treating as none");
            conn.send(&ServerMessage::error(
                "Client events are not enabled for this app",
                Some(4301),
            ));
            return;
        }
    };

    // Must have a channel
    let channel = match &msg.channel {
        Some(c) => c.clone(),
        None => {
            conn.send(&ServerMessage::error("No channel provided", Some(4009)));
            return;
        }
    };

    let kind = ChannelKind::from_name(&channel);
    if !kind.allows_client_events() {
        conn.send(&ServerMessage::error(
            "Client events are only allowed on private and presence channels",
            Some(4009),
        ));
        return;
    }

    // Must be subscribed to the channel
    if !conn.is_subscribed(&channel) {
        conn.send(&ServerMessage::error(
            "Not subscribed to channel",
            Some(4009),
        ));
        return;
    }

    // In "members" mode, only presence channel members can send client events
    if is_members_only && (!kind.is_presence() || conn.get_presence(&channel).is_none()) {
        conn.send(&ServerMessage::error(
            "Client events require presence channel membership",
            Some(4301),
        ));
        return;
    }

    // Rate limit check
    if !state
        .rate_limiter
        .check_client_event(&conn.socket_id, conn.app.max_client_events_per_sec)
    {
        conn.send(&ServerMessage::error("Rate limit exceeded", Some(4301)));
        return;
    }

    // Check event name length
    if msg.event.len() > conn.app.max_event_name_length {
        conn.send(&ServerMessage::error("Event name too long", Some(4009)));
        return;
    }

    // Include JSON escaping in the wire size.
    if msg.data.to_string().len() > conn.app.max_event_payload_in_kb.saturating_mul(1024) {
        conn.send(&ServerMessage::error("Event payload too large", Some(4009)));
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
