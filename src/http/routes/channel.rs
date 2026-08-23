use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::http::auth as http_auth;
use crate::protocol::channels::ChannelKind;
use crate::state::AppState;

pub async fn get_channel(
    Path((app_id, channel_name)): Path<(String, String)>,
    Query(params): Query<BTreeMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let path = format!("/apps/{}/channels/{}", app_id, channel_name);
    let app = match http_auth::authenticate_app(&state, &app_id, "GET", &path, &params, &[]) {
        Ok(a) => a,
        Err(e) => return e,
    };

    // Rate limit check
    if !state
        .rate_limiter
        .check_read_request(&app_id, app.max_read_req_per_sec)
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "Rate limit exceeded"})),
        );
    }

    let info_attrs: Vec<String> = params
        .get("info")
        .map(|s| s.split(',').map(|v| v.trim().to_string()).collect())
        .unwrap_or_default();
    let want_sub = info_attrs.iter().any(|v| v == "subscription_count");
    let want_usr = info_attrs.iter().any(|v| v == "user_count");

    if want_usr && !ChannelKind::from_name(&channel_name).is_presence() {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": "user_count is only available for presence channels"}),
            ),
        );
    }

    let count = state
        .adapter
        .get_channel_socket_count(&app_id, &channel_name)
        .await;
    let mut result = serde_json::json!({
        "occupied": count > 0,
    });
    if want_sub {
        result["subscription_count"] = serde_json::json!(count);
    }
    if want_usr {
        let user_count = state
            .adapter
            .get_presence_user_count(&app_id, &channel_name)
            .await;
        result["user_count"] = serde_json::json!(user_count);
    }

    (StatusCode::OK, Json(result))
}
