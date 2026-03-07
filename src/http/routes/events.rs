use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::http::auth as http_auth;
use crate::protocol::channels::ChannelKind;
use crate::protocol::messages::ServerMessage;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct TriggerRequest {
    pub name: String,
    pub data: String,
    #[serde(default)]
    pub channels: Option<Vec<String>>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub socket_id: Option<String>,
    #[serde(default)]
    pub info: Option<String>,
}

pub async fn trigger(
    Path(app_id): Path<String>,
    Query(params): Query<BTreeMap<String, String>>,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    // Find app
    let app = match state.app_manager.find_by_id(&app_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "App not found"}))),
    };

    // Verify signature
    let auth_signature = match params.get("auth_signature") {
        Some(s) => s.clone(),
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Missing auth_signature"}))),
    };

    let path = format!("/apps/{}/events", app_id);
    if !http_auth::verify_http_signature(&app.secret, "POST", &path, &params, &auth_signature) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Invalid signature"})));
    }

    // Verify body_md5 if provided
    if let Some(expected_md5) = params.get("body_md5") {
        let actual_md5 = http_auth::body_md5(&body);
        if &actual_md5 != expected_md5 {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "body_md5 mismatch"})));
        }
    }

    // Rate limit check
    if !state.rate_limiter.check_backend_event(&app_id, app.max_backend_events_per_sec) {
        return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({"error": "Rate limit exceeded"})));
    }

    // Parse body
    let req: TriggerRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": format!("Invalid body: {}", e)}))),
    };

    // Determine channels
    let channels: Vec<String> = if let Some(chs) = req.channels {
        chs
    } else if let Some(ch) = req.channel {
        vec![ch]
    } else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "No channels specified"})));
    };

    // Validate
    if channels.len() > app.max_event_channels_at_once {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Too many channels"})));
    }

    if req.name.len() > app.max_event_name_length {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Event name too long"})));
    }

    if req.data.len() > app.max_event_payload_in_kb * 1024 {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(serde_json::json!({"error": format!("The data content of this event exceeds the allowed maximum ({})", app.max_event_payload_in_kb * 1024)})));
    }

    state.metrics.on_http_call(body.len() as u64, 0);

    // Parse info parameter (from body or query string)
    let info_str = req.info
        .or_else(|| params.get("info").cloned());
    let want_subscription_count = info_str.as_deref()
        .map(|s| s.split(',').any(|v| v.trim() == "subscription_count"))
        .unwrap_or(false);
    let want_user_count = info_str.as_deref()
        .map(|s| s.split(',').any(|v| v.trim() == "user_count"))
        .unwrap_or(false);
    let want_info = want_subscription_count || want_user_count;

    // Send to each channel and collect info if requested
    let data_value = serde_json::Value::String(req.data);
    let mut channels_info = serde_json::Map::new();

    for channel in &channels {
        let msg = ServerMessage::channel_event(&req.name, channel, data_value.clone(), None);
        let msg_json = msg.to_json();

        // Cache for cache channels (only if app has caching enabled)
        let kind = ChannelKind::from_name(channel);
        if kind.is_cache() && app.enable_cache_channels {
            let cache_key = format!("cache:{}:{}", app_id, channel);
            state
                .cache
                .set(&cache_key, &msg_json, state.config.cache_ttl)
                .await;
        }

        state
            .adapter
            .send_to_channel(&app_id, channel, bytes::Bytes::from(msg_json), req.socket_id.as_deref())
            .await;

        // Collect channel info if requested
        if want_info {
            let mut ch_info = serde_json::Map::new();
            if want_subscription_count {
                let count = state.adapter.get_channel_socket_count(&app_id, channel).await;
                ch_info.insert("subscription_count".to_string(), serde_json::json!(count));
            }
            if want_user_count && kind.is_presence() {
                let count = state.adapter.get_presence_user_count(&app_id, channel).await;
                ch_info.insert("user_count".to_string(), serde_json::json!(count));
            }
            channels_info.insert(channel.clone(), serde_json::Value::Object(ch_info));
        }
    }

    if !channels_info.is_empty() {
        (StatusCode::OK, Json(serde_json::json!({"channels": channels_info})))
    } else {
        (StatusCode::OK, Json(serde_json::json!({})))
    }
}
