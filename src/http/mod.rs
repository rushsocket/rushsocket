pub mod auth;
pub mod routes;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};

use crate::state::AppState;

pub fn create_router(state: Arc<AppState>) -> Router {
    let config = &state.config;
    let event_limit = config.max_http_event_body_kb * 1024;
    let batch_limit = config.max_http_batch_body_kb * 1024;
    let concurrency = config.max_concurrent_http_requests;

    let x_request_id = HeaderName::from_static("x-request-id");

    let api_routes = Router::new()
        .route(
            "/apps/{app_id}/events",
            post(routes::events::trigger).layer(DefaultBodyLimit::max(event_limit)),
        )
        .route(
            "/apps/{app_id}/batch_events",
            post(routes::batch_events::trigger_batch).layer(DefaultBodyLimit::max(batch_limit)),
        )
        .route(
            "/apps/{app_id}/channels",
            get(routes::channels::list_channels),
        )
        .route(
            "/apps/{app_id}/channels/{channel_name}",
            get(routes::channel::get_channel),
        )
        .route(
            "/apps/{app_id}/channels/{channel_name}/users",
            get(routes::channel_users::get_users),
        )
        .route(
            "/apps/{app_id}/connections",
            get(routes::connections::get_connections),
        )
        .route(
            "/apps/{app_id}/users/{user_id}/terminate_connections",
            post(routes::terminate::terminate),
        );

    // Apply concurrency limit with load shedding to API routes only.
    // Health routes are never affected by the concurrency limit.
    let api_routes = if concurrency > 0 {
        api_routes.layer(
            ServiceBuilder::new()
                .layer(axum::error_handling::HandleErrorLayer::new(
                    |err: tower::BoxError| async move {
                        if err.is::<tower::load_shed::error::Overloaded>() {
                            StatusCode::SERVICE_UNAVAILABLE
                        } else {
                            StatusCode::INTERNAL_SERVER_ERROR
                        }
                    },
                ))
                .layer(tower::load_shed::LoadShedLayer::new())
                .layer(tower::limit::ConcurrencyLimitLayer::new(concurrency)),
        )
    } else {
        api_routes
    };

    let health_routes = Router::new()
        .route("/", get(routes::health::index))
        .route("/ready", get(routes::health::ready))
        .route("/accept-traffic", get(routes::health::accept_traffic));

    // Request IDs on all main-listener routes (health + API), metrics listener excluded.
    Router::new()
        .merge(health_routes)
        .merge(api_routes)
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(x_request_id.clone(), MakeRequestUuid))
                .layer(PropagateRequestIdLayer::new(x_request_id))
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}
