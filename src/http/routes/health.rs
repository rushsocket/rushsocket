use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::state::AppState;

pub async fn index() -> impl IntoResponse {
    "OK"
}

pub async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if state.is_ready() {
        (StatusCode::OK, "OK")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "Not ready")
    }
}

pub async fn accept_traffic(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if state.is_closing() {
        return (StatusCode::SERVICE_UNAVAILABLE, "Server is closing");
    }
    (StatusCode::OK, "OK")
}
