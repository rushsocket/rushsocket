use std::sync::Arc;

use crate::auth;
use crate::protocol::messages::{ClientMessage, ServerMessage};
use crate::state::AppState;
use crate::websocket::connection::{UserInfo, WsConnection};

fn extract_data(msg: &ClientMessage) -> Option<serde_json::Map<String, serde_json::Value>> {
    if let Some(obj) = msg.data.as_object() {
        return Some(obj.clone());
    }
    if let Some(s) = msg.data.as_str() {
        return serde_json::from_str(s).ok();
    }
    None
}

pub async fn handle(state: &Arc<AppState>, conn: &Arc<WsConnection>, msg: &ClientMessage) {
    if !conn.app.enable_user_authentication {
        conn.send(&ServerMessage::error(
            "User authentication is not enabled",
            Some(4009),
        ));
        return;
    }

    let data = match extract_data(msg) {
        Some(d) => d,
        None => {
            conn.send(&ServerMessage::error("Invalid signin data", Some(4009)));
            return;
        }
    };

    let auth_str = match data.get("auth").and_then(|v| v.as_str()) {
        Some(a) => a.to_string(),
        None => {
            conn.send(&ServerMessage::error("No auth provided", Some(4009)));
            return;
        }
    };

    let user_data_str = match data.get("user_data").and_then(|v| v.as_str()) {
        Some(d) => d.to_string(),
        None => {
            conn.send(&ServerMessage::error("No user_data provided", Some(4009)));
            return;
        }
    };

    if !auth::verify_signin_auth(
        &conn.app.key,
        &conn.app.secret,
        &conn.socket_id,
        &user_data_str,
        &auth_str,
    ) {
        conn.send(&ServerMessage::error(
            "Signin authentication failed",
            Some(4009),
        ));
        return;
    }

    let user_data: serde_json::Value = match serde_json::from_str(&user_data_str) {
        Ok(d) => d,
        Err(_) => {
            conn.send(&ServerMessage::error("Invalid user_data JSON", Some(4009)));
            return;
        }
    };

    let user_id = match user_data.get("id").and_then(|v| {
        v.as_str()
            .map(|s| s.to_string())
            .or_else(|| v.as_i64().map(|n| n.to_string()))
    }) {
        Some(id) => id,
        None => {
            conn.send(&ServerMessage::error(
                "user_data must contain id",
                Some(4009),
            ));
            return;
        }
    };

    let user_id_clone = user_id.clone();
    *conn.user.lock() = Some(UserInfo {
        id: user_id,
        user_data: user_data.clone(),
    });
    conn.user_authenticated.store(true, std::sync::atomic::Ordering::Relaxed);

    // Track user->socket mapping for terminate API
    state.adapter.add_user(&conn.app.id, &user_id_clone, &conn.socket_id).await;

    conn.send(&ServerMessage::signin_success(&user_data_str));
}
