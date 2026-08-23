use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::http::auth as http_auth;
use crate::state::AppState;

pub async fn terminate(
    Path((app_id, user_id)): Path<(String, String)>,
    Query(params): Query<BTreeMap<String, String>>,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    let path = format!("/apps/{}/users/{}/terminate_connections", app_id, user_id);
    let app = match http_auth::authenticate_app(&state, &app_id, "POST", &path, &params, &body) {
        Ok(a) => a,
        Err(e) => return e,
    };

    if !state
        .rate_limiter
        .check_backend_event(&app_id, app.max_backend_events_per_sec)
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "Rate limit exceeded"})),
        );
    }

    state
        .adapter
        .terminate_user_connections(&app_id, &user_id)
        .await;

    (StatusCode::OK, Json(serde_json::json!({})))
}
