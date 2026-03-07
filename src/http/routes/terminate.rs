use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use crate::http::auth as http_auth;
use crate::state::AppState;

pub async fn terminate(
    Path((app_id, user_id)): Path<(String, String)>,
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

    let path = format!("/apps/{}/users/{}/terminate_connections", app_id, user_id);
    if !http_auth::verify_http_signature(&app.secret, "POST", &path, &params, &auth_signature) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Invalid signature"})));
    }

    state.adapter.terminate_user_connections(&app_id, &user_id).await;

    (StatusCode::OK, Json(serde_json::json!({})))
}
