pub mod connection;
pub mod events;
pub mod lifecycle;
pub mod socket_id;

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tracing::{debug, info_span, Instrument};

use crate::protocol::error_codes;
use crate::protocol::messages::ServerMessage;
use crate::state::AppState;

use self::connection::WsConnection;

/// Check if an origin matches an allowed origin pattern.
/// Supports exact match and wildcard `*` (matches everything).
fn origin_matches(origin: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Case-insensitive exact match
    origin.eq_ignore_ascii_case(pattern)
}

/// Validate the origin header against the app's allowed origins.
/// If allowed_origins is empty, all origins are accepted.
fn validate_origin(origin: Option<&str>, allowed_origins: &[String]) -> bool {
    if allowed_origins.is_empty() {
        return true;
    }
    let origin = match origin {
        Some(o) => o,
        None => return false,
    };
    allowed_origins.iter().any(|p| origin_matches(origin, p))
}

/// WebSocket upgrade handler at `/app/{key}`.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(key): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    ws.read_buffer_size(4096)
        .write_buffer_size(4096)
        .max_write_buffer_size(65536)
        .on_upgrade(move |socket| handle_socket(socket, key, origin, state))
}

async fn handle_socket(
    socket: WebSocket,
    key: String,
    origin: Option<String>,
    state: Arc<AppState>,
) {
    // Check if server is closing
    if state.is_closing() {
        return;
    }

    // Look up app by key
    let app = match state.app_manager.find_by_key(&key) {
        Some(app) => app,
        None => {
            let (mut sink, _) = socket.split();
            let err = ServerMessage::error(
                "Could not find app",
                Some(error_codes::UNKNOWN_APP),
            );
            let _ = sink.send(Message::Text(err.to_json().into())).await;
            let _ = sink.close().await;
            return;
        }
    };

    // Check if app is enabled
    if !app.enabled {
        let (mut sink, _) = socket.split();
        let err = ServerMessage::error(
            "Application is disabled",
            Some(error_codes::APP_DISABLED),
        );
        let _ = sink.send(Message::Text(err.to_json().into())).await;
        let _ = sink.close().await;
        return;
    }

    // Validate origin
    if !validate_origin(origin.as_deref(), &app.allowed_origins) {
        let (mut sink, _) = socket.split();
        let err = ServerMessage::error(
            "Origin not allowed",
            Some(4009),
        );
        let _ = sink.send(Message::Text(err.to_json().into())).await;
        let _ = sink.close().await;
        return;
    }

    let app_id = app.id.clone();

    // Check memory threshold (uses cached value from stats timer, no /proc reads)
    if state.config.memory_threshold_percent > 0.0 {
        let usage = state.cached_memory_percent();
        if usage >= state.config.memory_threshold_percent {
            let (mut sink, _) = socket.split();
            let err = ServerMessage::error(
                "Memory usage too high",
                Some(4009),
            );
            let _ = sink.send(Message::Text(err.to_json().into())).await;
            let _ = sink.close().await;
            return;
        }
    }

    // Generate socket ID
    let socket_id: Arc<str> = socket_id::generate().into();

    // Run the connection loop inside a connection-scoped tracing span
    let span = info_span!(
        "ws_connection",
        socket_id = %socket_id,
        app_id = %app_id,
    );

    run_connection(socket, app, app_id, socket_id, state)
        .instrument(span)
        .await;
}

async fn run_connection(
    socket: WebSocket,
    app: Arc<crate::app::App>,
    app_id: String,
    socket_id: Arc<str>,
    state: Arc<AppState>,
) {
    // Create bounded channel for outbound messages (slow consumers get disconnected)
    let (tx, mut rx) = mpsc::channel::<bytes::Bytes>(512);

    // Create connection
    let conn = Arc::new(WsConnection::new(socket_id.clone(), app.clone(), tx));

    // Atomically check connection limit and register (no TOCTOU race)
    if !state.adapter.try_add_socket(&app_id, conn.clone(), app.max_connections).await {
        let (mut sink, _) = socket.split();
        let err = ServerMessage::error(
            "Over connection quota",
            Some(error_codes::APP_OVER_CONNECTION_QUOTA),
        );
        let _ = sink.send(Message::Text(err.to_json().into())).await;
        let _ = sink.close().await;
        return;
    }
    state.metrics.on_connect(&app_id);

    // Split socket
    let (mut sink, mut stream) = socket.split();

    // Send connection_established
    let established = ServerMessage::connection_established(&socket_id, state.config.activity_timeout as u32);
    let established_json = established.to_json();
    state.metrics.on_ws_message_sent(&app_id, established_json.len() as u64);
    if sink.send(Message::Text(established_json.into())).await.is_err() {
        state.adapter.remove_socket(&app_id, &socket_id).await;
        state.metrics.on_disconnect(&app_id);
        return;
    }

    debug!("connection established");

    let ping_interval_secs = state.config.server_ping_interval;
    let pong_timeout_secs = state.config.server_pong_timeout;
    let max_message_bytes = app.max_message_size_in_kb * 1024;

    // Single select! loop: handles inbound messages, outbound sends, and activity timeout.
    // This eliminates 2 spawned tasks per connection (send_task + activity_timer).
    //
    // Activity timer uses a resettable sleep:
    //   - Normally sleeps for ping_interval (e.g. 60s)
    //   - After sending a ping, resets to pong_timeout (e.g. 15s)
    //   - On any activity, resets back to ping_interval
    let activity_deadline = tokio::time::sleep(std::time::Duration::from_secs(ping_interval_secs));
    tokio::pin!(activity_deadline);

    loop {
        // Check if this connection was flagged as a slow consumer
        if conn.should_close() {
            break;
        }

        tokio::select! {
            result = stream.next() => {
                match result {
                    Some(Ok(Message::Text(text))) => {
                        if text.len() > max_message_bytes {
                            conn.send(&ServerMessage::error("Message too large", Some(4009)));
                            break;
                        }
                        conn.clear_pinged();
                        activity_deadline.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_secs(ping_interval_secs));
                        state.metrics.on_ws_message_received(&app_id, text.len() as u64);
                        lifecycle::on_message(&state, &conn, &text).await;
                    }
                    Some(Ok(Message::Binary(data))) => {
                        if data.len() > max_message_bytes {
                            conn.send(&ServerMessage::error("Message too large", Some(4009)));
                            break;
                        }
                        conn.clear_pinged();
                        activity_deadline.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_secs(ping_interval_secs));
                        state.metrics.on_ws_message_received(&app_id, data.len() as u64);
                        if let Ok(text) = std::str::from_utf8(&data) {
                            lifecycle::on_message(&state, &conn, text).await;
                        }
                    }
                    Some(Ok(Message::Pong(_) | Message::Ping(_))) => {
                        conn.clear_pinged();
                        activity_deadline.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_secs(ping_interval_secs));
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                }
            }
            Some(msg) = rx.recv() => {
                state.metrics.on_ws_message_sent(&app_id, msg.len() as u64);
                // msg is Bytes (valid UTF-8 JSON); Utf8Bytes::try_from is zero-copy
                match axum::extract::ws::Utf8Bytes::try_from(msg) {
                    Ok(utf8) => {
                        if sink.send(Message::Text(utf8)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break, // should never happen — all messages are JSON
                }
            }
            _ = &mut activity_deadline => {
                if conn.is_stale() {
                    // Sent a ping and waited pong_timeout with no response — disconnect
                    let err = ServerMessage::error(
                        "Pong reply not received",
                        Some(error_codes::ACTIVITY_TIMEOUT),
                    );
                    let _ = sink.send(Message::Text(err.to_json().into())).await;
                    break;
                } else {
                    // No activity for ping_interval — send a server ping
                    conn.mark_pinged();
                    if sink.send(Message::Text(crate::protocol::messages::PING_JSON.into())).await.is_err() {
                        break;
                    }
                    // Wait pong_timeout for a response
                    activity_deadline.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_secs(pong_timeout_secs));
                }
            }
        }
    }

    // Cleanup
    let _ = sink.close().await;
    lifecycle::on_close(&state, &conn).await;
    state.metrics.on_disconnect(&app_id);

    debug!("connection closed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_origin_matches_wildcard() {
        assert!(origin_matches("https://example.com", "*"));
    }

    #[test]
    fn test_origin_matches_exact() {
        assert!(origin_matches("https://example.com", "https://example.com"));
        assert!(!origin_matches("https://other.com", "https://example.com"));
    }

    #[test]
    fn test_origin_matches_case_insensitive() {
        assert!(origin_matches("https://Example.COM", "https://example.com"));
    }

    #[test]
    fn test_validate_origin_empty_allows_all() {
        assert!(validate_origin(None, &[]));
        assert!(validate_origin(Some("https://any.com"), &[]));
    }

    #[test]
    fn test_validate_origin_rejects_missing() {
        let allowed = vec!["https://example.com".to_string()];
        assert!(!validate_origin(None, &allowed));
    }

    #[test]
    fn test_validate_origin_accepts_listed() {
        let allowed = vec![
            "https://example.com".to_string(),
            "https://other.com".to_string(),
        ];
        assert!(validate_origin(Some("https://example.com"), &allowed));
        assert!(validate_origin(Some("https://other.com"), &allowed));
        assert!(!validate_origin(Some("https://evil.com"), &allowed));
    }
}
