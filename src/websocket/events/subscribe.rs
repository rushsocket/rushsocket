use std::sync::Arc;

use crate::auth;
use crate::protocol::channels::{self, ChannelKind};
use crate::protocol::messages::{ClientMessage, ServerMessage};
use crate::state::AppState;
use crate::webhook::WebhookEvent;
use crate::websocket::connection::WsConnection;

fn send_subscription_error(conn: &WsConnection, channel: &str, message: &str, status: u16) {
    conn.send(&ServerMessage::subscription_error(channel, message, status));
}

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
        None => {
            conn.send(&ServerMessage::error("Invalid data", Some(4009)));
            return;
        }
    };

    let channel = match data.get("channel").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => {
            conn.send(&ServerMessage::error("No channel provided", Some(4009)));
            return;
        }
    };

    // Validate channel name
    if !channels::is_valid_channel_name(&channel)
        || channel.len() > conn.app.max_channel_name_length
    {
        send_subscription_error(conn, &channel, "Invalid channel name", 403);
        return;
    }

    let kind = ChannelKind::from_name(&channel);
    let app_id = &conn.app.id;

    // Already subscribed: re-ack without adapter add, webhooks, or cache replay.
    if conn.is_subscribed(&channel) {
        if kind.is_presence() {
            let members = state.adapter.get_presence_members(app_id, &channel).await;
            let ids: Vec<String> = members.keys().cloned().collect();
            let hash = serde_json::json!(members);
            conn.send(&ServerMessage::presence_subscription_succeeded(
                &channel,
                &ids,
                &hash,
                members.len(),
            ));
        } else {
            conn.send(&ServerMessage::subscription_succeeded(&channel));
        }
        return;
    }

    match kind {
        ChannelKind::Public | ChannelKind::CachePublic => {
            let occupied = state
                .adapter
                .add_to_channel(app_id, &conn.socket_id, &channel)
                .await;
            conn.add_channel(channel.clone());
            conn.send(&ServerMessage::subscription_succeeded(&channel));

            if state
                .adapter
                .is_channel_globally_occupied(app_id, &channel, occupied)
                .await
                && let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id))
            {
                wh.send(WebhookEvent::channel_occupied(&channel));
            }
            crate::webhook::send_subscription_count(
                state,
                app_id,
                &channel,
                conn.app.enable_subscription_count_webhook,
            )
            .await;

            // Replay cached event for cache channels
            if kind.is_cache() {
                replay_cache(state, conn, &channel).await;
            }
        }
        ChannelKind::Private
        | ChannelKind::EncryptedPrivate
        | ChannelKind::CachePrivate
        | ChannelKind::CacheEncryptedPrivate => {
            let auth_str = match data.get("auth").and_then(|v| v.as_str()) {
                Some(a) => a,
                None => {
                    send_subscription_error(conn, &channel, "Authorization required", 401);
                    return;
                }
            };

            if !auth::verify_channel_auth(
                &conn.app.key,
                &conn.app.secret,
                &conn.socket_id,
                &channel,
                None,
                auth_str,
            ) {
                send_subscription_error(conn, &channel, "Authorization failed", 401);
                return;
            }

            let occupied = state
                .adapter
                .add_to_channel(app_id, &conn.socket_id, &channel)
                .await;
            conn.add_channel(channel.clone());
            conn.send(&ServerMessage::subscription_succeeded(&channel));

            if state
                .adapter
                .is_channel_globally_occupied(app_id, &channel, occupied)
                .await
                && let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id))
            {
                wh.send(WebhookEvent::channel_occupied(&channel));
            }
            crate::webhook::send_subscription_count(
                state,
                app_id,
                &channel,
                conn.app.enable_subscription_count_webhook,
            )
            .await;

            if kind.is_cache() {
                replay_cache(state, conn, &channel).await;
            }
        }
        ChannelKind::Presence | ChannelKind::CachePresence => {
            let auth_str = match data.get("auth").and_then(|v| v.as_str()) {
                Some(a) => a,
                None => {
                    send_subscription_error(conn, &channel, "Authorization required", 401);
                    return;
                }
            };

            let channel_data_str = match data.get("channel_data").and_then(|v| v.as_str()) {
                Some(d) => d.to_string(),
                None => {
                    send_subscription_error(
                        conn,
                        &channel,
                        "Presence channel requires channel_data",
                        403,
                    );
                    return;
                }
            };

            if !auth::verify_channel_auth(
                &conn.app.key,
                &conn.app.secret,
                &conn.socket_id,
                &channel,
                Some(&channel_data_str),
                auth_str,
            ) {
                send_subscription_error(conn, &channel, "Authorization failed", 401);
                return;
            }

            let channel_data: serde_json::Value = match serde_json::from_str(&channel_data_str) {
                Ok(d) => d,
                Err(_) => {
                    send_subscription_error(conn, &channel, "Invalid channel_data JSON", 403);
                    return;
                }
            };

            let user_id = match channel_data.get("user_id").and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            }) {
                Some(id) => id,
                None => {
                    send_subscription_error(
                        conn,
                        &channel,
                        "channel_data must contain user_id",
                        403,
                    );
                    return;
                }
            };

            let user_info = channel_data
                .get("user_info")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            // Check member size limit
            let member_json = serde_json::to_string(&channel_data).unwrap_or_default();
            if member_json.len() > conn.app.max_presence_member_size_in_kb * 1024 {
                send_subscription_error(conn, &channel, "Member data too large", 403);
                return;
            }

            // Atomically check presence member limit and add (no TOCTOU race).
            // In Redis mode, the member snapshot is fetched cluster-wide after the local insert.
            let (is_new_member, members) = match state
                .adapter
                .try_add_presence_member(
                    app_id,
                    &channel,
                    &conn.socket_id,
                    &user_id,
                    &user_info,
                    conn.app.max_presence_members_per_channel,
                )
                .await
            {
                Ok(result) => result,
                Err(()) => {
                    send_subscription_error(conn, &channel, "Over presence member limit", 403);
                    return;
                }
            };

            let occupied = state
                .adapter
                .add_to_channel(app_id, &conn.socket_id, &channel)
                .await;

            conn.add_channel(channel.clone());
            conn.set_presence(channel.clone(), user_id.clone(), user_info.clone());

            if state
                .adapter
                .is_channel_globally_occupied(app_id, &channel, occupied)
                .await
                && let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id))
            {
                wh.send(WebhookEvent::channel_occupied(&channel));
            }

            let ids: Vec<String> = members.keys().cloned().collect();
            let hash = serde_json::json!(members);

            conn.send(&ServerMessage::presence_subscription_succeeded(
                &channel,
                &ids,
                &hash,
                members.len(),
            ));

            if is_new_member {
                let added = ServerMessage::member_added(&channel, &user_id, &user_info);
                state
                    .adapter
                    .send_to_channel(
                        app_id,
                        &channel,
                        bytes::Bytes::from(added.to_json()),
                        Some(&conn.socket_id),
                    )
                    .await;

                if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id)) {
                    wh.send(WebhookEvent::member_added(&channel, &user_id));
                }
            }

            if kind.is_cache() {
                replay_cache(state, conn, &channel).await;
            }
        }
    }
}

/// Replay the last cached event for a cache channel, or send cache_miss.
async fn replay_cache(state: &Arc<AppState>, conn: &Arc<WsConnection>, channel: &str) {
    if !conn.app.enable_cache_channels {
        return;
    }
    let cache_key = format!("cache:{}:{}", conn.app.id, channel);
    match state.cache.get(&cache_key).await {
        Some(cached) => {
            conn.send_raw(bytes::Bytes::from(cached));
        }
        None => {
            conn.send(&ServerMessage::cache_miss(channel));
            if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(&conn.app.id)) {
                wh.send(WebhookEvent::cache_miss(channel));
            }
        }
    }
}
