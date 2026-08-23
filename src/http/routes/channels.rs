use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::http::auth as http_auth;
use crate::protocol::channels::ChannelKind;
use crate::state::AppState;

pub async fn list_channels(
    Path(app_id): Path<String>,
    Query(params): Query<BTreeMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let path = format!("/apps/{}/channels", app_id);
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
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({"error": "user_count is only available for presence channels (use filter_by_prefix=presence-)"}),
                ),
            );
        }
    }

    let channels = state.adapter.get_channels(&app_id).await;
    let filtered: Vec<String> = channels
        .into_iter()
        .filter(|channel| {
            filter_prefix
                .as_ref()
                .map(|prefix| channel.starts_with(prefix.as_str()))
                .unwrap_or(true)
        })
        .collect();

    let want_sub = info_attrs.iter().any(|v| v == "subscription_count");
    let counts = if want_sub {
        state
            .adapter
            .get_channel_socket_counts(&app_id, &filtered)
            .await
    } else {
        std::collections::HashMap::new()
    };

    let mut result = serde_json::Map::new();

    for channel in filtered {
        let mut info = serde_json::Map::new();

        if want_sub {
            let count = counts.get(&channel).copied().unwrap_or(0);
            info.insert("subscription_count".to_string(), serde_json::json!(count));
        }

        if info_attrs.contains(&"user_count".to_string()) {
            let kind = ChannelKind::from_name(&channel);
            if kind.is_presence() {
                let count = state
                    .adapter
                    .get_presence_user_count(&app_id, &channel)
                    .await;
                info.insert("user_count".to_string(), serde_json::json!(count));
            }
        }

        result.insert(channel, serde_json::Value::Object(info));
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({"channels": result})),
    )
}
