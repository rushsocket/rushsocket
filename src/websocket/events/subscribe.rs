use std::sync::Arc;

use crate::auth;
use crate::protocol::channels::{self, ChannelKind};
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
        conn.send(&ServerMessage::error("Invalid channel name", Some(4009)));
        return;
    }

    // Already subscribed?
    if conn.is_subscribed(&channel) {
        return;
    }

    let kind = ChannelKind::from_name(&channel);
    let app_id = &conn.app.id;

    match kind {
        ChannelKind::Public | ChannelKind::CachePublic => {
            let occupied = state
                .adapter
                .add_to_channel(app_id, &conn.socket_id, &channel)
                .await;
            conn.add_channel(channel.clone());
            conn.send(&ServerMessage::subscription_succeeded(&channel));

            if state.adapter.is_channel_globally_occupied(app_id, &channel, occupied).await {
                if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id)) {
                    wh.send(WebhookEvent::channel_occupied(&channel));
                }
            }
            crate::webhook::send_subscription_count(state, app_id, &channel, conn.app.enable_subscription_count_webhook).await;

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
                    conn.send(&ServerMessage::error("Authorization required", Some(4009)));
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
                conn.send(&ServerMessage::error("Authorization failed", Some(4009)));
                return;
            }

            let occupied = state
                .adapter
                .add_to_channel(app_id, &conn.socket_id, &channel)
                .await;
            conn.add_channel(channel.clone());
            conn.user_authenticated.store(true, std::sync::atomic::Ordering::Relaxed);
            conn.send(&ServerMessage::subscription_succeeded(&channel));

            if state.adapter.is_channel_globally_occupied(app_id, &channel, occupied).await {
                if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id)) {
                    wh.send(WebhookEvent::channel_occupied(&channel));
                }
            }
            crate::webhook::send_subscription_count(state, app_id, &channel, conn.app.enable_subscription_count_webhook).await;

            if kind.is_cache() {
                replay_cache(state, conn, &channel).await;
            }
        }
        ChannelKind::Presence | ChannelKind::CachePresence => {
            let auth_str = match data.get("auth").and_then(|v| v.as_str()) {
                Some(a) => a,
                None => {
                    conn.send(&ServerMessage::error("Authorization required", Some(4009)));
                    return;
                }
            };

            let channel_data_str = match data.get("channel_data").and_then(|v| v.as_str()) {
                Some(d) => d.to_string(),
                None => {
                    conn.send(&ServerMessage::error(
                        "Presence channel requires channel_data",
                        Some(4009),
                    ));
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
                conn.send(&ServerMessage::error("Authorization failed", Some(4009)));
                return;
            }

            let channel_data: serde_json::Value = match serde_json::from_str(&channel_data_str) {
                Ok(d) => d,
                Err(_) => {
                    conn.send(&ServerMessage::error("Invalid channel_data JSON", Some(4009)));
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
                    conn.send(&ServerMessage::error(
                        "channel_data must contain user_id",
                        Some(4009),
                    ));
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
                conn.send(&ServerMessage::error("Member data too large", Some(4009)));
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
                    conn.send(&ServerMessage::error(
                        "Over presence member limit",
                        Some(4009),
                    ));
                    return;
                }
            };

            let occupied = state
                .adapter
                .add_to_channel(app_id, &conn.socket_id, &channel)
                .await;

            conn.add_channel(channel.clone());
            conn.set_presence(channel.clone(), user_id.clone(), user_info.clone());
            conn.user_authenticated.store(true, std::sync::atomic::Ordering::Relaxed);

            if let Some(wh) = state.webhooks.as_ref().and_then(|w| w.get(app_id)) {
                if occupied {
                    wh.send(WebhookEvent::channel_occupied(&channel));
                }
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
                    .send_to_channel(app_id, &channel, bytes::Bytes::from(added.to_json()), Some(&conn.socket_id))
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

