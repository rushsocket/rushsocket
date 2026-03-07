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
pub struct BatchEvent {
    pub name: String,
    pub data: String,
    pub channel: String,
    #[serde(default)]
    pub socket_id: Option<String>,
    #[serde(default)]
    pub info: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BatchRequest {
    pub batch: Vec<BatchEvent>,
}

pub async fn trigger_batch(
    Path(app_id): Path<String>,
    Query(params): Query<BTreeMap<String, String>>,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    let app = match state.app_manager.find_by_id(&app_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "App not found"}))),
    };

    let auth_signature = match params.get("auth_signature") {
        Some(s) => s.clone(),
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Missing auth_signature"}))),
    };

    let path = format!("/apps/{}/batch_events", app_id);
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

    let req: BatchRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": format!("Invalid body: {}", e)}))),
    };

    if req.batch.len() > app.max_event_batch_size {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Batch too large"})));
    }

    // Per-event payload size check
    for event in &req.batch {
        if event.data.len() > app.max_event_payload_in_kb * 1024 {
            return (StatusCode::PAYLOAD_TOO_LARGE, Json(serde_json::json!({"error": format!("The data content of this event exceeds the allowed maximum ({})", app.max_event_payload_in_kb * 1024)})));
        }
    }

    state.metrics.on_http_call(body.len() as u64, 0);

    // Global info param from query string
    let global_info = params.get("info").cloned();

    let mut batch_results = Vec::new();
    let mut has_any_info = false;

    for event in &req.batch {
        let data_value = serde_json::Value::String(event.data.clone());
        let msg = ServerMessage::channel_event(&event.name, &event.channel, data_value, None);
        let msg_json = msg.to_json();

        // Cache for cache channels (only if app has caching enabled)
        let kind = ChannelKind::from_name(&event.channel);
        if kind.is_cache() && app.enable_cache_channels {
            let cache_key = format!("cache:{}:{}", app_id, event.channel);
            state
                .cache
                .set(&cache_key, &msg_json, state.config.cache_ttl)
                .await;
        }

        state
            .adapter
            .send_to_channel(
                &app_id,
                &event.channel,
                bytes::Bytes::from(msg_json),
                event.socket_id.as_deref(),
            )
            .await;

        // Collect info if requested (per-event or global)
        let info_param = event.info.as_deref().or(global_info.as_deref());
        if let Some(info_str) = info_param {
            has_any_info = true;
            let want_sub = info_str.split(',').any(|v| v.trim() == "subscription_count");
            let want_usr = info_str.split(',').any(|v| v.trim() == "user_count");
            let mut ch_info = serde_json::Map::new();
            if want_sub {
                let count = state.adapter.get_channel_socket_count(&app_id, &event.channel).await;
                ch_info.insert("subscription_count".to_string(), serde_json::json!(count));
            }
            if want_usr && kind.is_presence() {
                let count = state.adapter.get_presence_user_count(&app_id, &event.channel).await;
                ch_info.insert("user_count".to_string(), serde_json::json!(count));
            }
            batch_results.push(serde_json::Value::Object(ch_info));
        } else {
            batch_results.push(serde_json::json!({}));
        }
    }

    if has_any_info {
        (StatusCode::OK, Json(serde_json::json!({"batch": batch_results})))
    } else {
        (StatusCode::OK, Json(serde_json::json!({})))
    }
}
