use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use crate::http::auth as http_auth;
use crate::protocol::channels::ChannelKind;
use crate::state::AppState;

pub async fn list_channels(
    Path(app_id): Path<String>,
    Query(params): Query<BTreeMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let app = match state.app_manager.find_by_id(&app_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "App not found"}))),
    };

    let auth_signature = match params.get("auth_signature") {
        Some(s) => s.clone(),
        None => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Missing auth_signature"}))),
    };

    let path = format!("/apps/{}/channels", app_id);
    if !http_auth::verify_http_signature(&app.secret, "GET", &path, &params, &auth_signature) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Invalid signature"})));
    }

    // Rate limit check
    if !state.rate_limiter.check_read_request(&app_id, app.max_read_req_per_sec) {
        return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({"error": "Rate limit exceeded"})));
    }

    let filter_prefix = params.get("filter_by_prefix").cloned();
    let info_attrs: Vec<String> = params
        .get("info")
        .map(|s| s.split(',').map(|v| v.trim().to_string()).collect())
        .unwrap_or_default();

    // Pusher compat: user_count is only allowed when filtering by presence- prefix
    if info_attrs.contains(&"user_count".to_string()) {
        let is_presence_prefix = filter_prefix
            .as_deref()
            .map(|p| p.starts_with("presence-"))
            .unwrap_or(false);
        if !is_presence_prefix {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "user_count is only available for presence channels (use filter_by_prefix=presence-)"})));
        }
    }

    let channels = state.adapter.get_channels(&app_id).await;
    let mut result = serde_json::Map::new();

    for channel in channels {
        if let Some(ref prefix) = filter_prefix {
            if !channel.starts_with(prefix.as_str()) {
                continue;
            }
        }

        let mut info = serde_json::Map::new();

        if info_attrs.contains(&"subscription_count".to_string()) {
            let count = state.adapter.get_channel_socket_count(&app_id, &channel).await;
            info.insert("subscription_count".to_string(), serde_json::json!(count));
        }

        if info_attrs.contains(&"user_count".to_string()) {
            let kind = ChannelKind::from_name(&channel);
            if kind.is_presence() {
                let count = state.adapter.get_presence_user_count(&app_id, &channel).await;
                info.insert("user_count".to_string(), serde_json::json!(count));
            }
        }

        result.insert(channel, serde_json::Value::Object(info));
    }

    (StatusCode::OK, Json(serde_json::json!({"channels": result})))
}
