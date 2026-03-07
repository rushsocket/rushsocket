use std::sync::Arc;

use crate::protocol::channels::ChannelKind;
use crate::protocol::messages::{ClientMessage, ServerMessage};
use crate::state::AppState;
use crate::webhook::WebhookEvent;

use super::connection::WsConnection;
use super::events;

pub async fn on_message(state: &Arc<AppState>, conn: &Arc<WsConnection>, text: &str) {
    let msg: ClientMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(_) => {
            conn.send(&ServerMessage::error("Invalid JSON", Some(4009)));
            return;
        }
    };

    match msg.event.as_str() {
        "pusher:ping" => events::ping_pong::handle(conn),
        "pusher:subscribe" => events::subscribe::handle(state, conn, &msg).await,
        "pusher:unsubscribe" => events::unsubscribe::handle(state, conn, &msg).await,
        "pusher:signin" => events::signin::handle(state, conn, &msg).await,
        event if event.starts_with("client-") => {
            events::client_event::handle(state, conn, &msg).await;
        }
        _ => {
            conn.send(&ServerMessage::error(
                &format!("Unknown event: {}", msg.event),
                Some(4009),
            ));
        }
    }
}

pub async fn on_close(state: &Arc<AppState>, conn: &Arc<WsConnection>) {
    let app_id = &conn.app.id;
    let socket_id = &conn.socket_id;

    // Get all channels this socket was subscribed to
    let channels = conn.get_channels();

    for channel in &channels {
        let kind = ChannelKind::from_name(channel);

        // Handle presence member removal
        if kind.is_presence() {
            if let Some((user_id, _)) = conn.get_presence(channel) {
                let user_gone = state
                    .adapter
                    .remove_presence_member(app_id, channel, socket_id, &user_id)
                    .await;
                if user_gone {
                    let msg = ServerMessage::member_removed(channel, &user_id);
                    state
                        .adapter
                        .send_to_channel(app_id, channel, bytes::Bytes::from(msg.to_json()), Some(socket_id))
                        .await;

                    if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id)) {
                        wh.send(WebhookEvent::member_removed(channel, &user_id));
                    }
                }
            }
        }

        let vacated = state
            .adapter
            .remove_from_channel(app_id, socket_id, channel)
            .await;

        if state.adapter.is_channel_globally_vacated(app_id, channel, vacated).await {
            if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id)) {
                wh.send(WebhookEvent::channel_vacated(channel));
            }
        }

        crate::webhook::send_subscription_count(
            state,
            app_id,
            channel,
            conn.app.enable_subscription_count_webhook,
        )
        .await;
    }

    // Remove user->socket mapping if user was authenticated
    let user_id = conn.user.lock().as_ref().map(|u| u.id.clone());
    if let Some(user_id) = user_id {
        state.adapter.remove_user(app_id, &user_id, socket_id).await;
    }

    // Remove socket from adapter
    state.adapter.remove_socket(app_id, socket_id).await;

    // Clean up rate limiter state for this socket
    state.rate_limiter.remove_socket(socket_id);
}
