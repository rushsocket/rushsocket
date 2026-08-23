//! In-process protocol and REST tests (local adapter).
//! Redis live tests live under `src/adapter/redis` and skip unless `REDIS_URL` is set.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use rushsocket::config::Config;
use rushsocket::server::build_app;
use rushsocket::state::AppState;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const SECRET: &str = "rushsocket-secret";
const KEY: &str = "rushsocket-key";
const APP_ID: &str = "rushsocket-id";

struct TestServer {
    http: String,
    ws: String,
    state: Arc<AppState>,
}

async fn spawn_server(config: Config) -> TestServer {
    let (app, state) = build_app(config).await.expect("build_app");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    TestServer {
        http: format!("http://{addr}"),
        ws: format!("ws://{addr}/app/{KEY}"),
        state,
    }
}

async fn connect_ws(url: &str) -> (Value, Ws) {
    let (mut ws, _) = connect_async(url).await.expect("ws connect");
    let msg = recv_json(&mut ws).await;
    assert_eq!(msg["event"], "pusher:connection_established");
    (msg, ws)
}

async fn recv_json(ws: &mut Ws) -> Value {
    let next = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .expect("recv timeout")
        .expect("stream closed")
        .expect("ws error");
    match next {
        Message::Text(t) => serde_json::from_str(&t).expect("json"),
        other => panic!("unexpected frame {other:?}"),
    }
}

fn socket_id_from_established(msg: &Value) -> String {
    let data = msg["data"].as_str().expect("double-encoded data");
    let inner: Value = serde_json::from_str(data).unwrap();
    inner["socket_id"].as_str().unwrap().to_string()
}

fn sign_query(method: &str, path: &str, extra: &[(&str, String)]) -> String {
    let mut params = BTreeMap::new();
    params.insert("auth_key".to_string(), KEY.to_string());
    params.insert(
        "auth_timestamp".to_string(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string(),
    );
    params.insert("auth_version".to_string(), "1.0".to_string());
    for (k, v) in extra {
        params.insert((*k).to_string(), v.clone());
    }
    let qs: String = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let sig = rushsocket::auth::sign_http_request(SECRET, method, path, &qs);
    format!("{qs}&auth_signature={sig}")
}

fn channel_auth(socket_id: &str, channel: &str) -> String {
    let sig = rushsocket::auth::sign(SECRET, &format!("{socket_id}:{channel}"));
    format!("{KEY}:{sig}")
}

#[tokio::test]
async fn pong_is_not_unknown_event() {
    let srv = spawn_server(Config::default()).await;
    let (_est, mut ws) = connect_ws(&srv.ws).await;
    ws.send(Message::Text(r#"{"event":"pusher:pong"}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Text(
        r#"{"event":"pusher:subscribe","data":{"channel":"public-ok"}}"#.into(),
    ))
    .await
    .unwrap();
    let msg = recv_json(&mut ws).await;
    assert_eq!(msg["event"], "pusher_internal:subscription_succeeded");
    assert_eq!(msg["channel"], "public-ok");
}

#[tokio::test]
async fn subscribe_private_without_auth_is_subscription_error_not_4009() {
    let srv = spawn_server(Config::default()).await;
    let (_est, mut ws) = connect_ws(&srv.ws).await;
    ws.send(Message::Text(
        r#"{"event":"pusher:subscribe","data":{"channel":"private-secret"}}"#.into(),
    ))
    .await
    .unwrap();
    let err = recv_json(&mut ws).await;
    assert_eq!(err["event"], "pusher:subscription_error");
    assert_eq!(err["channel"], "private-secret");
    ws.send(Message::Text(
        r#"{"event":"pusher:subscribe","data":{"channel":"public-ok"}}"#.into(),
    ))
    .await
    .unwrap();
    let ok = recv_json(&mut ws).await;
    assert_eq!(ok["event"], "pusher_internal:subscription_succeeded");
}

#[tokio::test]
async fn resubscribe_reacks_without_error() {
    let srv = spawn_server(Config::default()).await;
    let (_est, mut ws) = connect_ws(&srv.ws).await;
    let sub = r#"{"event":"pusher:subscribe","data":{"channel":"public-ok"}}"#;
    ws.send(Message::Text(sub.into())).await.unwrap();
    let first = recv_json(&mut ws).await;
    assert_eq!(first["event"], "pusher_internal:subscription_succeeded");
    ws.send(Message::Text(sub.into())).await.unwrap();
    let second = recv_json(&mut ws).await;
    assert_eq!(second["event"], "pusher_internal:subscription_succeeded");
}

#[tokio::test]
async fn connection_quota_uses_4004() {
    let mut cfg = Config::default();
    cfg.apps[0].max_connections = 1;
    let srv = spawn_server(cfg).await;
    let (_est, _hold) = connect_ws(&srv.ws).await;
    let (mut ws2, _) = connect_async(&srv.ws).await.expect("second connect");
    let msg = recv_json(&mut ws2).await;
    assert_eq!(msg["event"], "pusher:error");
    let data: Value = serde_json::from_str(msg["data"].as_str().unwrap()).unwrap();
    assert_eq!(data["code"], 4004);
}

#[tokio::test]
async fn client_event_on_public_is_rejected() {
    let srv = spawn_server(Config::default()).await;
    let (_est, mut ws) = connect_ws(&srv.ws).await;
    ws.send(Message::Text(
        r#"{"event":"pusher:subscribe","data":{"channel":"public-ok"}}"#.into(),
    ))
    .await
    .unwrap();
    let _ = recv_json(&mut ws).await;
    ws.send(Message::Text(
        r#"{"event":"client-hello","channel":"public-ok","data":{}}"#.into(),
    ))
    .await
    .unwrap();
    let err = recv_json(&mut ws).await;
    assert_eq!(err["event"], "pusher:error");
}

#[tokio::test]
async fn client_event_on_private_is_forwarded() {
    let srv = spawn_server(Config::default()).await;
    let (est_a, mut a) = connect_ws(&srv.ws).await;
    let (est_b, mut b) = connect_ws(&srv.ws).await;
    let sid_a = socket_id_from_established(&est_a);
    let sid_b = socket_id_from_established(&est_b);
    let auth_a = channel_auth(&sid_a, "private-chat");
    let auth_b = channel_auth(&sid_b, "private-chat");
    a.send(Message::Text(
        format!(
            r#"{{"event":"pusher:subscribe","data":{{"channel":"private-chat","auth":"{auth_a}"}}}}"#
        )
        .into(),
    ))
    .await
    .unwrap();
    b.send(Message::Text(
        format!(
            r#"{{"event":"pusher:subscribe","data":{{"channel":"private-chat","auth":"{auth_b}"}}}}"#
        )
        .into(),
    ))
    .await
    .unwrap();
    assert_eq!(
        recv_json(&mut a).await["event"],
        "pusher_internal:subscription_succeeded"
    );
    assert_eq!(
        recv_json(&mut b).await["event"],
        "pusher_internal:subscription_succeeded"
    );
    a.send(Message::Text(
        r#"{"event":"client-hello","channel":"private-chat","data":"hi"}"#.into(),
    ))
    .await
    .unwrap();
    let fwd = recv_json(&mut b).await;
    assert_eq!(fwd["event"], "client-hello");
    assert_eq!(fwd["channel"], "private-chat");
}

#[tokio::test]
async fn cache_name_families_are_independent_keys() {
    let srv = spawn_server(Config::default()).await;
    let body_a = r#"{"name":"e","channel":"private-cache-foo","data":"{\"v\":1}"}"#;
    let body_b = r#"{"name":"e","channel":"cache-private-foo","data":"{\"v\":2}"}"#;
    trigger(&srv.http, body_a).await;
    trigger(&srv.http, body_b).await;

    let (est, mut ws) = connect_ws(&srv.ws).await;
    let sid = socket_id_from_established(&est);
    let auth = channel_auth(&sid, "private-cache-foo");
    ws.send(Message::Text(
        format!(
            r#"{{"event":"pusher:subscribe","data":{{"channel":"private-cache-foo","auth":"{auth}"}}}}"#
        )
        .into(),
    ))
    .await
    .unwrap();
    let sub = recv_json(&mut ws).await;
    assert_eq!(sub["event"], "pusher_internal:subscription_succeeded");
    let replay = recv_json(&mut ws).await;
    assert_eq!(replay["event"], "e");
    assert_eq!(replay["channel"], "private-cache-foo");
}

#[tokio::test]
async fn closing_emits_4200() {
    let srv = spawn_server(Config::default()).await;
    let (_est, mut ws) = connect_ws(&srv.ws).await;
    srv.state.set_closing();
    let msg = recv_json(&mut ws).await;
    assert_eq!(msg["event"], "pusher:error");
    let data: Value = serde_json::from_str(msg["data"].as_str().unwrap()).unwrap();
    assert_eq!(data["code"], 4200);
}

#[tokio::test]
async fn closing_rejects_new_ws_with_503() {
    let srv = spawn_server(Config::default()).await;
    srv.state.set_closing();
    let err = connect_async(&srv.ws).await;
    assert!(err.is_err());
}

#[tokio::test]
async fn rest_empty_channels_is_400() {
    let srv = spawn_server(Config::default()).await;
    let body = r#"{"name":"e","channels":[],"data":"{}"}"#;
    let resp = trigger_status(&srv.http, body).await;
    assert_eq!(resp, 400);
}

#[tokio::test]
async fn rest_invalid_channel_name_is_400() {
    let srv = spawn_server(Config::default()).await;
    let body = r#"{"name":"e","channel":"has spaces","data":"{}"}"#;
    let resp = trigger_status(&srv.http, body).await;
    assert_eq!(resp, 400);
}

#[tokio::test]
async fn rest_missing_timestamp_is_401() {
    let srv = spawn_server(Config::default()).await;
    let url = format!(
        "{}/apps/{APP_ID}/channels?auth_key={KEY}&auth_version=1.0&auth_signature=00",
        srv.http
    );
    let status = reqwest::get(&url).await.unwrap().status();
    assert_eq!(status.as_u16(), 401);
}

fn encode_query_values_for_wire(qs: &str) -> String {
    qs.split('&')
        .map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let k = parts.next().unwrap_or("");
            let v = parts.next().unwrap_or("");
            format!("{k}={}", v.replace('@', "%40").replace(',', "%2C"))
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[tokio::test]
async fn rest_hmac_accepts_percent_encoded_query_signed_over_decoded() {
    let srv = spawn_server(Config::default()).await;
    let path = format!("/apps/{APP_ID}/channels");
    let qs = sign_query(
        "GET",
        &path,
        &[("filter_by_prefix", "@foo,bar".to_string())],
    );
    assert!(
        qs.contains("filter_by_prefix=@foo,bar"),
        "sign over decoded values: {qs}"
    );
    let wire = encode_query_values_for_wire(&qs);
    assert!(
        wire.contains("filter_by_prefix=%40foo%2Cbar"),
        "wire must percent-encode: {wire}"
    );
    let status = reqwest::get(format!("{}{path}?{wire}", srv.http))
        .await
        .unwrap()
        .status();
    assert_eq!(status.as_u16(), 200);
}

#[tokio::test]
async fn rest_batch_event_name_too_long_is_400() {
    let srv = spawn_server(Config::default()).await;
    let long_name = "e".repeat(300);
    let body = format!(r#"{{"batch":[{{"name":"{long_name}","channel":"c","data":"{{}}"}}]}}"#);
    let path = format!("/apps/{APP_ID}/batch_events");
    let md5 = rushsocket::http::auth::body_md5(body.as_bytes());
    let qs = sign_query("POST", &path, &[("body_md5", md5)]);
    let status = reqwest::Client::new()
        .post(format!("{}{path}?{qs}", srv.http))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(status, 400);
}

#[tokio::test]
async fn rest_get_channel_default_is_occupied_only() {
    let srv = spawn_server(Config::default()).await;
    let (_est, mut ws) = connect_ws(&srv.ws).await;
    ws.send(Message::Text(
        r#"{"event":"pusher:subscribe","data":{"channel":"pub-info"}}"#.into(),
    ))
    .await
    .unwrap();
    let _ = recv_json(&mut ws).await;

    let path = format!("/apps/{APP_ID}/channels/pub-info");
    let qs = sign_query("GET", &path, &[]);
    let body: Value = reqwest::get(format!("{}{path}?{qs}", srv.http))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["occupied"], true);
    assert!(body.get("subscription_count").is_none());
    assert!(body.get("user_count").is_none());

    let qs_info = sign_query("GET", &path, &[("info", "subscription_count".to_string())]);
    let body_info: Value = reqwest::get(format!("{}{path}?{qs_info}", srv.http))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body_info["occupied"], true);
    assert_eq!(body_info["subscription_count"], 1);
}

fn signin_frame(socket_id: &str, user_id: &str) -> String {
    let user_data = format!(r#"{{"id":"{user_id}"}}"#);
    let sig = rushsocket::auth::sign(SECRET, &format!("{socket_id}::user::{user_data}"));
    serde_json::json!({
        "event": "pusher:signin",
        "data": { "auth": format!("{KEY}:{sig}"), "user_data": user_data }
    })
    .to_string()
}

#[tokio::test]
async fn signin_removes_previous_user_mapping_when_id_changes() {
    let srv = spawn_server(Config::default()).await;
    let (est, mut ws) = connect_ws(&srv.ws).await;
    let socket_id = socket_id_from_established(&est);

    ws.send(Message::Text(signin_frame(&socket_id, "user-a").into()))
        .await
        .unwrap();
    let ok = recv_json(&mut ws).await;
    assert_eq!(ok["event"], "pusher:signin_success");

    ws.send(Message::Text(signin_frame(&socket_id, "user-a").into()))
        .await
        .unwrap();
    let _ = recv_json(&mut ws).await;
    let still_a = srv
        .state
        .adapter
        .get_sockets_for_user(APP_ID, "user-a")
        .await;
    assert_eq!(still_a.len(), 1);

    ws.send(Message::Text(signin_frame(&socket_id, "user-b").into()))
        .await
        .unwrap();
    let _ = recv_json(&mut ws).await;
    let after_a = srv
        .state
        .adapter
        .get_sockets_for_user(APP_ID, "user-a")
        .await;
    let after_b = srv
        .state
        .adapter
        .get_sockets_for_user(APP_ID, "user-b")
        .await;
    assert!(after_a.is_empty(), "old user mapping must be removed");
    assert_eq!(after_b.len(), 1);
}

async fn trigger(base: &str, body: &str) {
    let status = trigger_status(base, body).await;
    assert_eq!(status, 200, "trigger {body}");
}

async fn trigger_status(base: &str, body: &str) -> u16 {
    let path = format!("/apps/{APP_ID}/events");
    let md5 = rushsocket::http::auth::body_md5(body.as_bytes());
    let qs = sign_query("POST", &path, &[("body_md5", md5)]);
    reqwest::Client::new()
        .post(format!("{base}{path}?{qs}"))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}
