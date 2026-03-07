use std::sync::Arc;

use bytes::Bytes;

use crate::protocol::messages;
use crate::websocket::connection::WsConnection;

pub fn handle(conn: &Arc<WsConnection>) {
    conn.send_raw(Bytes::from_static(messages::PONG_JSON.as_bytes()));
}
