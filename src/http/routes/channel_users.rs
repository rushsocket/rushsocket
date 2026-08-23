use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::http::auth as http_auth;
use crate::protocol::channels::ChannelKind;
use crate::state::AppState;

pub async fn get_users(
    Path((app_id, channel_name)): Path<(String, String)>,
    Query(params): Query<BTreeMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let path = format!("/apps/{}/channels/{}/users", app_id, channel_name);
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

    let kind = ChannelKind::from_name(&channel_name);
    if !kind.is_presence() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Not a presence channel"})),
        );
    }

    let members = state
        .adapter
        .get_presence_members(&app_id, &channel_name)
        .await;
    let users: Vec<serde_json::Value> = members
        .keys()
        .map(|uid| serde_json::json!({"id": uid}))
        .collect();

    (StatusCode::OK, Json(serde_json::json!({"users": users})))
}
