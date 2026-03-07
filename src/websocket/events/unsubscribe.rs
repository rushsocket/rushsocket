use std::sync::Arc;

use crate::protocol::channels::ChannelKind;
use crate::protocol::messages::{ClientMessage, ServerMessage};
use crate::state::AppState;
use crate::webhook::WebhookEvent;
use crate::websocket::connection::WsConnection;

fn extract_data(msg: &ClientMessage) -> Option<serde_json::Map<String, serde_json::Value>> {
    if let Some(obj) = msg.data.as_object() {
        return Some(obj.clone());
    }
    if let Some(s) = msg.data.as_str() {
        return serde_json::from_str(s).ok();
    }
    None
}

pub async fn handle(state: &Arc<AppState>, conn: &Arc<WsConnection>, msg: &ClientMessage) {
    let data = match extract_data(msg) {
        Some(d) => d,
        None => return,
    };

    let channel = match data.get("channel").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => return,
    };

    if !conn.is_subscribed(&channel) {
        return;
    }

    let app_id = &conn.app.id;
    let kind = ChannelKind::from_name(&channel);

    // Handle presence member removal
    if kind.is_presence() {
        if let Some((user_id, _)) = conn.remove_presence(&channel) {
            let user_gone = state
                .adapter
                .remove_presence_member(app_id, &channel, &conn.socket_id, &user_id)
                .await;
            if user_gone {
                let removed = ServerMessage::member_removed(&channel, &user_id);
                state
                    .adapter
                    .send_to_channel(app_id, &channel, bytes::Bytes::from(removed.to_json()), Some(&conn.socket_id))
                    .await;

                if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id)) {
                    wh.send(WebhookEvent::member_removed(&channel, &user_id));
                }
            }
        }
    }

    conn.remove_channel(&channel);
    let vacated = state
        .adapter
        .remove_from_channel(app_id, &conn.socket_id, &channel)
        .await;

    if state.adapter.is_channel_globally_vacated(app_id, &channel, vacated).await {
        if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id)) {
            wh.send(WebhookEvent::channel_vacated(&channel));
        }
    }

    crate::webhook::send_subscription_count(
        state,
        app_id,
        &channel,
        conn.app.enable_subscription_count_webhook,
    )
    .await;
}
