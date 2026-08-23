use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::http::auth as http_auth;
use crate::state::AppState;

pub async fn get_connections(
    Path(app_id): Path<String>,
    Query(params): Query<BTreeMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let path = format!("/apps/{}/connections", app_id);
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

    let count = state.adapter.get_connection_count(&app_id).await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "connections": count
        })),
    )
}
