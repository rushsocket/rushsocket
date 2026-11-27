//! Real-world Pusher-client simulation.
//!
//! Local adapter always runs. Redis adapter runs when `REDIS_URL` is set or a
//! server is listening on `127.0.0.1:6379`.
//!
//! Scenarios model a chat/presence app: REST triggers, private client events,
//! presence membership, cache replay, concurrent join/leave, quota races,
//! soak, throughput, and (Redis) two-node cross-broadcast.

use std::collections::BTreeMap;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::http::StatusCode;
use axum::routing::post;
use futures_util::{SinkExt, StreamExt};
use rushsocket::config::Config;
use rushsocket::server::build_app;
use rushsocket::state::AppState;
use serde_json::{Value, json};
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
    #[allow(dead_code)]
    state: Arc<AppState>,
}

struct Client {
    ws: Ws,
    socket_id: String,
}

fn redis_url() -> Option<String> {
    if let Ok(u) = std::env::var("REDIS_URL")
        && !u.is_empty()
    {
        return Some(u);
    }
    TcpStream::connect(("127.0.0.1", 6379))
        .ok()
        .map(|_| "redis://127.0.0.1:6379".to_string())
}

fn local_cfg() -> Config {
    Config::default()
}

#[allow(clippy::field_reassign_with_default)]
fn redis_cfg(url: &str, prefix: &str) -> Config {
    let mut cfg = Config::default();
    cfg.adapter = "redis".into();
    cfg.redis.url = url.to_string();
    cfg.redis.prefix = prefix.to_string();
    cfg.redis.request_timeout = 250;
    cfg
}

async fn wait_ready(state: &AppState) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if state.is_ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    panic!(
        "adapter not ready after 5s (adapter={})",
        state.config.adapter
    );
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_test_writer()
        .try_init();
}

async fn spawn_server(cfg: Config) -> TestServer {
    init_tracing();
    let (app, state) = build_app(cfg).await.expect("build_app");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for _ in 0..80 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    wait_ready(&state).await;
    TestServer {
        http: format!("http://{addr}"),
        ws: format!("ws://{addr}/app/{KEY}"),
        state,
    }
}

async fn spawn_local() -> TestServer {
    spawn_server(local_cfg()).await
}

async fn spawn_redis() -> Option<TestServer> {
    let url = redis_url()?;
    let prefix = format!("sim-{}", uuid::Uuid::new_v4());
    Some(spawn_server(redis_cfg(&url, &prefix)).await)
}

#[allow(clippy::field_reassign_with_default)]
fn redis_cluster_cfg(url: &str, prefix: &str) -> Config {
    let mut cfg = redis_cfg(url, prefix);
    cfg.redis.request_timeout = 1500;
    cfg
}

async fn spawn_redis_pair() -> Option<(TestServer, TestServer)> {
    let url = redis_url()?;
    let prefix = format!("sim-pair-{}", uuid::Uuid::new_v4());
    let a = spawn_server(redis_cluster_cfg(&url, &prefix)).await;
    let b = spawn_server(redis_cluster_cfg(&url, &prefix)).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    Some((a, b))
}

async fn recv_json_timeout(ws: &mut Ws, timeout: Duration) -> Value {
    let next = tokio::time::timeout(timeout, ws.next())
        .await
        .expect("recv timeout")
        .expect("stream closed")
        .expect("ws error");
    match next {
        Message::Text(t) => serde_json::from_str(&t).expect("json"),
        other => panic!("unexpected frame {other:?}"),
    }
}

async fn recv_json(ws: &mut Ws) -> Value {
    recv_json_timeout(ws, Duration::from_secs(5)).await
}

async fn recv_event(ws: &mut Ws, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        let msg = recv_json_timeout(ws, left.max(Duration::from_millis(50))).await;
        if msg["event"] == name {
            return msg;
        }
    }
    panic!("did not receive event {name}");
}

fn socket_id_from_established(msg: &Value) -> String {
    let data = msg["data"].as_str().expect("double-encoded data");
    let inner: Value = serde_json::from_str(data).unwrap();
    inner["socket_id"].as_str().unwrap().to_string()
}

impl Client {
    async fn connect(url: &str) -> Self {
        let (mut ws, _) = connect_async(url).await.expect("ws connect");
        let msg = recv_json(&mut ws).await;
        assert_eq!(
            msg["event"], "pusher:connection_established",
            "handshake: {msg}"
        );
        let socket_id = socket_id_from_established(&msg);
        Self { ws, socket_id }
    }

    async fn subscribe_public(&mut self, channel: &str) {
        let frame = json!({"event":"pusher:subscribe","data":{"channel": channel}});
        self.ws
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
        let ack = recv_event(&mut self.ws, "pusher_internal:subscription_succeeded").await;
        assert_eq!(ack["channel"], channel);
    }

    async fn subscribe_private(&mut self, channel: &str) {
        let auth = channel_auth(&self.socket_id, channel, None);
        let frame = json!({"event":"pusher:subscribe","data":{"channel": channel, "auth": auth}});
        self.ws
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
        let ack = recv_event(&mut self.ws, "pusher_internal:subscription_succeeded").await;
        assert_eq!(ack["channel"], channel);
    }

    async fn subscribe_presence(&mut self, channel: &str, user_id: &str) -> Value {
        let channel_data = json!({"user_id": user_id, "user_info": {"name": user_id}}).to_string();
        let auth = channel_auth(&self.socket_id, channel, Some(&channel_data));
        let frame = json!({
            "event":"pusher:subscribe",
            "data": {"channel": channel, "auth": auth, "channel_data": channel_data}
        });
        self.ws
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
        recv_event(&mut self.ws, "pusher_internal:subscription_succeeded").await
    }

    async fn unsubscribe(&mut self, channel: &str) {
        let frame = json!({"event":"pusher:unsubscribe","data":{"channel": channel}});
        self.ws
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
    }

    async fn client_event(&mut self, channel: &str, event: &str, data: Value) {
        let frame = json!({"event": event, "channel": channel, "data": data});
        self.ws
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
    }
}

fn channel_auth(socket_id: &str, channel: &str, channel_data: Option<&str>) -> String {
    let msg = match channel_data {
        Some(d) => format!("{socket_id}:{channel}:{d}"),
        None => format!("{socket_id}:{channel}"),
    };
    let sig = rushsocket::auth::sign(SECRET, &msg);
    format!("{KEY}:{sig}")
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

async fn rest_get(base: &str, path: &str, extra: &[(&str, String)]) -> (u16, Value) {
    let qs = sign_query("GET", path, extra);
    let resp = reqwest::get(format!("{base}{path}?{qs}")).await.unwrap();
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(json!({}));
    (status, body)
}

async fn trigger(base: &str, name: &str, channel: &str, data: &str) -> u16 {
    let path = format!("/apps/{APP_ID}/events");
    let body = json!({"name": name, "channel": channel, "data": data}).to_string();
    let md5 = rushsocket::http::auth::body_md5(body.as_bytes());
    let qs = sign_query("POST", &path, &[("body_md5", md5)]);
    reqwest::Client::new()
        .post(format!("{base}{path}?{qs}"))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn channel_info(base: &str, channel: &str, info: &str) -> Value {
    let path = format!("/apps/{APP_ID}/channels/{channel}");
    let extra = if info.is_empty() {
        vec![]
    } else {
        vec![("info", info.to_string())]
    };
    let (status, body) = rest_get(base, &path, &extra).await;
    assert_eq!(status, 200, "GET channel {channel}: {body}");
    body
}

async fn connection_count(base: &str) -> usize {
    let path = format!("/apps/{APP_ID}/connections");
    let (status, body) = rest_get(base, &path, &[]).await;
    assert_eq!(status, 200, "{body}");
    body["connections"].as_u64().unwrap() as usize
}

async fn spawn_webhook_sink() -> (String, Arc<Mutex<Vec<Value>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let app = Router::new().route(
        "/hooks",
        post(move |body: Bytes| {
            let captured = captured.clone();
            async move {
                if let Ok(v) = serde_json::from_slice::<Value>(&body)
                    && let Some(arr) = v.get("events").and_then(|e| e.as_array())
                {
                    captured.lock().unwrap().extend(arr.iter().cloned());
                }
                StatusCode::OK
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/hooks"), events)
}

fn drain_named(events: &Mutex<Vec<Value>>, name: &str, channel: &str) -> usize {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == name && e["channel"] == channel)
        .count()
}

async fn wait_hooks(events: &Mutex<Vec<Value>>, name: &str, channel: &str, min: usize) {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if drain_named(events, name, channel) >= min {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "webhook {name}/{channel} expected >= {min}, got {}",
                drain_named(events, name, channel)
            );
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

// --- scenarios (adapter-agnostic) ---

async fn scenario_chat_fanout(srv: &TestServer) {
    const N: usize = 12;
    let mut clients = Vec::new();
    for _ in 0..N {
        let mut c = Client::connect(&srv.ws).await;
        c.subscribe_public("public-lobby").await;
        clients.push(c);
    }

    let info = channel_info(&srv.http, "public-lobby", "subscription_count").await;
    assert_eq!(info["occupied"], true);
    assert_eq!(info["subscription_count"], N);

    assert_eq!(connection_count(&srv.http).await, N);
    assert_eq!(
        trigger(&srv.http, "message", "public-lobby", r#"{"t":1}"#).await,
        200
    );

    for c in &mut clients {
        let ev = recv_event(&mut c.ws, "message").await;
        assert_eq!(ev["channel"], "public-lobby");
    }

    clients.pop().unwrap().unsubscribe("public-lobby").await;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let info = channel_info(&srv.http, "public-lobby", "subscription_count").await;
        if info["subscription_count"] == N - 1 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("count after unsubscribe: {info}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn scenario_private_typing(srv: &TestServer) {
    let mut a = Client::connect(&srv.ws).await;
    let mut b = Client::connect(&srv.ws).await;
    a.subscribe_private("private-dm").await;
    b.subscribe_private("private-dm").await;

    a.client_event("private-dm", "client-typing", json!({"from":"a"}))
        .await;
    let fwd = recv_event(&mut b.ws, "client-typing").await;
    assert_eq!(fwd["channel"], "private-dm");

    let idle = tokio::time::timeout(Duration::from_millis(200), a.ws.next()).await;
    assert!(idle.is_err(), "sender must not receive own client event");
}

async fn scenario_presence_room(srv: &TestServer) {
    let mut alice = Client::connect(&srv.ws).await;
    let mut bob = Client::connect(&srv.ws).await;
    let ack_a = alice.subscribe_presence("presence-room", "alice").await;
    let data_a: Value = serde_json::from_str(ack_a["data"].as_str().unwrap()).unwrap();
    assert_eq!(data_a["presence"]["count"], 1);

    let ack_b = bob.subscribe_presence("presence-room", "bob").await;
    let data_b: Value = serde_json::from_str(ack_b["data"].as_str().unwrap()).unwrap();
    assert_eq!(data_b["presence"]["count"], 2);

    let added = recv_event(&mut alice.ws, "pusher_internal:member_added").await;
    let added_data: Value = serde_json::from_str(added["data"].as_str().unwrap()).unwrap();
    assert_eq!(added_data["user_id"], "bob");

    let path = format!("/apps/{APP_ID}/channels/presence-room/users");
    let (status, users) = rest_get(&srv.http, &path, &[]).await;
    assert_eq!(status, 200);
    let list = users["users"].as_array().unwrap();
    let ids: Vec<&str> = list.iter().filter_map(|u| u["id"].as_str()).collect();
    assert!(ids.contains(&"alice") && ids.contains(&"bob"), "{users}");

    bob.unsubscribe("presence-room").await;
    let removed = recv_event(&mut alice.ws, "pusher_internal:member_removed").await;
    let removed_data: Value = serde_json::from_str(removed["data"].as_str().unwrap()).unwrap();
    assert_eq!(removed_data["user_id"], "bob");
}

async fn scenario_cache_replay(srv: &TestServer) {
    assert_eq!(
        trigger(&srv.http, "tick", "private-cache-feed", r#"{"n":7}"#).await,
        200
    );
    let mut c = Client::connect(&srv.ws).await;
    c.subscribe_private("private-cache-feed").await;
    let replay = recv_event(&mut c.ws, "tick").await;
    assert_eq!(replay["channel"], "private-cache-feed");
}

async fn scenario_join_leave_race(srv: &TestServer) {
    const N: usize = 24;
    let mut joins = Vec::new();
    for i in 0..N {
        let url = srv.ws.clone();
        joins.push(tokio::spawn(async move {
            let mut c = Client::connect(&url).await;
            c.subscribe_public("public-race").await;
            c
        }));
        let _ = i;
    }
    let mut clients = Vec::new();
    for j in joins {
        clients.push(j.await.unwrap());
    }
    let info = channel_info(&srv.http, "public-race", "subscription_count").await;
    assert_eq!(info["subscription_count"], N, "{info}");

    let keep = clients.pop().unwrap();
    let leaves: Vec<_> = clients
        .into_iter()
        .map(|mut c| {
            tokio::spawn(async move {
                c.unsubscribe("public-race").await;
                c
            })
        })
        .collect();
    for l in leaves {
        let _ = l.await.unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let info = channel_info(&srv.http, "public-race", "subscription_count").await;
        if info["occupied"] == true && info["subscription_count"] == 1 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("after concurrent leave expected 1 occupant: {info}");
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    drop(keep);
}

async fn scenario_quota_race(mut cfg: Config) {
    cfg.apps[0].max_connections = 8;
    let srv = spawn_server(cfg).await;
    let url = srv.ws.clone();
    let mut tasks = Vec::new();
    for _ in 0..24 {
        let url = url.clone();
        tasks.push(tokio::spawn(async move {
            let (mut ws, _) = connect_async(&url).await.expect("connect");
            let msg = recv_json(&mut ws).await;
            if msg["event"] == "pusher:connection_established" {
                Ok(ws)
            } else {
                let data: Value = serde_json::from_str(msg["data"].as_str().unwrap()).unwrap();
                Err(data["code"].as_u64().unwrap())
            }
        }));
    }
    let mut hold = Vec::new();
    let mut ok = 0usize;
    let mut quota = 0usize;
    for t in tasks {
        match t.await.unwrap() {
            Ok(ws) => {
                ok += 1;
                hold.push(ws);
            }
            Err(4004) => quota += 1,
            Err(other) => panic!("unexpected reject code {other}"),
        }
    }
    assert_eq!(ok, 8, "admitted={ok} quota={quota}");
    assert_eq!(quota, 16, "admitted={ok} quota={quota}");
    drop(hold);
}

async fn scenario_soak(srv: &TestServer) {
    let mut live = Vec::new();
    for _ in 0..10 {
        let mut c = Client::connect(&srv.ws).await;
        c.subscribe_public("public-soak").await;
        live.push(c);
    }
    for round in 0..4u32 {
        assert_eq!(
            trigger(
                &srv.http,
                "pulse",
                "public-soak",
                &format!(r#"{{"r":{round}}}"#)
            )
            .await,
            200
        );
        for c in &mut live {
            let ev = recv_event(&mut c.ws, "pulse").await;
            assert_eq!(ev["channel"], "public-soak");
        }
        if round == 1 {
            live.truncate(6);
            for _ in 0..4 {
                let mut c = Client::connect(&srv.ws).await;
                c.subscribe_public("public-soak").await;
                live.push(c);
            }
        }
    }
    assert_eq!(live.len(), 10);
}

async fn scenario_throughput(srv: &TestServer, label: &str) {
    const CLIENTS: usize = 16;
    const EVENTS: usize = 40;
    let mut clients = Vec::new();
    for _ in 0..CLIENTS {
        let mut c = Client::connect(&srv.ws).await;
        c.subscribe_public("public-bench").await;
        clients.push(c);
    }
    let t0 = Instant::now();
    for i in 0..EVENTS {
        assert_eq!(
            trigger(
                &srv.http,
                "tick",
                "public-bench",
                &format!(r#"{{"i":{i}}}"#)
            )
            .await,
            200
        );
    }
    for c in &mut clients {
        let mut n = 0usize;
        let deadline = Instant::now() + Duration::from_secs(8);
        while n < EVENTS {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "{label} client missed ticks ({n}/{EVENTS})"
            );
            let msg = recv_json_timeout(&mut c.ws, left).await;
            if msg["event"] == "tick" {
                n += 1;
            }
        }
    }
    let elapsed = t0.elapsed();
    let deliveries = CLIENTS * EVENTS;
    let per_sec = deliveries as f64 / elapsed.as_secs_f64();
    eprintln!(
        "sim throughput {label}: {CLIENTS} clients × {EVENTS} events = {deliveries} frames in {:.3}s ({per_sec:.0} frames/s)",
        elapsed.as_secs_f64()
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "{label} too slow: {elapsed:?}"
    );
}

fn scale_n() -> usize {
    std::env::var("SIM_CONNECTIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(5000)
}

async fn scenario_scale(srv: &TestServer, n: usize, label: &str) {
    // macOS somaxconn is 128; stay under the listen backlog.
    const BATCH: usize = 32;
    let url = srv.ws.clone();
    let mut clients = Vec::with_capacity(n);
    let t_connect = Instant::now();
    for start in (0..n).step_by(BATCH) {
        let end = (start + BATCH).min(n);
        let mut futs = Vec::with_capacity(end - start);
        for _ in start..end {
            let url = url.clone();
            futs.push(tokio::spawn(async move {
                let (mut ws, _) = connect_async(&url).await.expect("ws connect");
                let est = recv_json_timeout(&mut ws, Duration::from_secs(20)).await;
                assert_eq!(est["event"], "pusher:connection_established", "{est}");
                let mut c = Client {
                    ws,
                    socket_id: socket_id_from_established(&est),
                };
                let frame = json!({"event":"pusher:subscribe","data":{"channel":"public-5k"}});
                c.ws.send(Message::Text(frame.to_string().into()))
                    .await
                    .unwrap();
                let ack = recv_json_timeout(&mut c.ws, Duration::from_secs(20)).await;
                assert_eq!(
                    ack["event"], "pusher_internal:subscription_succeeded",
                    "{ack}"
                );
                c
            }));
        }
        for fut in futs {
            clients.push(fut.await.expect("connect task"));
        }
    }
    let connect_elapsed = t_connect.elapsed();
    assert_eq!(clients.len(), n);

    let counted = connection_count(&srv.http).await;
    assert_eq!(counted, n, "{label} GET /connections");
    let info = channel_info(&srv.http, "public-5k", "subscription_count").await;
    assert_eq!(info["occupied"], true);
    assert_eq!(info["subscription_count"], n, "{label} subscription_count");

    let t_fanout = Instant::now();
    assert_eq!(
        trigger(&srv.http, "blast", "public-5k", r#"{"scale":5000}"#).await,
        200
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    for (i, c) in clients.iter_mut().enumerate() {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "{label} fan-out stalled at client {i}/{n}");
        loop {
            let ev = recv_json_timeout(&mut c.ws, left).await;
            if ev["event"] == "pusher:ping" {
                continue;
            }
            assert_eq!(ev["event"], "blast", "client {i}: {ev}");
            assert_eq!(ev["channel"], "public-5k");
            break;
        }
    }
    let fanout_elapsed = t_fanout.elapsed();
    let cps = n as f64 / connect_elapsed.as_secs_f64();
    let fps = n as f64 / fanout_elapsed.as_secs_f64();
    eprintln!(
        "sim scale {label}: {n} connections+subscribes in {:.3}s ({cps:.0} conn/s); fan-out {n} frames in {:.3}s ({fps:.0} frames/s)",
        connect_elapsed.as_secs_f64(),
        fanout_elapsed.as_secs_f64()
    );
}

// --- local ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_chat_fanout() {
    scenario_chat_fanout(&spawn_local().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_private_typing() {
    scenario_private_typing(&spawn_local().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_presence_room() {
    scenario_presence_room(&spawn_local().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_cache_replay() {
    scenario_cache_replay(&spawn_local().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_join_leave_race() {
    scenario_join_leave_race(&spawn_local().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_quota_race() {
    scenario_quota_race(local_cfg()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_soak() {
    scenario_soak(&spawn_local().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_throughput() {
    scenario_throughput(&spawn_local().await, "local").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_occupancy_webhook() {
    let (hook_url, events) = spawn_webhook_sink().await;
    let mut cfg = local_cfg();
    cfg.apps[0].webhook_url = Some(hook_url);
    cfg.apps[0].webhook_batch_ms = 40;
    let srv = spawn_server(cfg).await;
    let mut c = Client::connect(&srv.ws).await;
    c.subscribe_public("public-local-occ").await;
    wait_hooks(&events, "channel_occupied", "public-local-occ", 1).await;
    c.ws.close(None).await.ok();
    wait_hooks(&events, "channel_vacated", "public-local-occ", 1).await;
}

// --- redis ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_one_node_occupancy_webhook() {
    let Some(url) = redis_url() else {
        eprintln!("skipping redis_one_node_occupancy_webhook");
        return;
    };
    let (hook_url, events) = spawn_webhook_sink().await;
    let mut cfg = redis_cfg(&url, &format!("sim-one-{}", uuid::Uuid::new_v4()));
    cfg.apps[0].webhook_url = Some(hook_url);
    cfg.apps[0].webhook_batch_ms = 40;
    let srv = spawn_server(cfg).await;
    let mut c = Client::connect(&srv.ws).await;
    c.subscribe_public("public-one").await;
    wait_hooks(&events, "channel_occupied", "public-one", 1).await;
    c.ws.close(None).await.ok();
    wait_hooks(&events, "channel_vacated", "public-one", 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_chat_fanout() {
    let Some(srv) = spawn_redis().await else {
        eprintln!("skipping redis_chat_fanout (no Redis on 6379 / REDIS_URL)");
        return;
    };
    scenario_chat_fanout(&srv).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_private_typing() {
    let Some(srv) = spawn_redis().await else {
        eprintln!("skipping redis_private_typing");
        return;
    };
    scenario_private_typing(&srv).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_presence_room() {
    let Some(srv) = spawn_redis().await else {
        eprintln!("skipping redis_presence_room");
        return;
    };
    scenario_presence_room(&srv).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_cache_replay() {
    let Some(srv) = spawn_redis().await else {
        eprintln!("skipping redis_cache_replay");
        return;
    };
    scenario_cache_replay(&srv).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_join_leave_race() {
    let Some(srv) = spawn_redis().await else {
        eprintln!("skipping redis_join_leave_race");
        return;
    };
    scenario_join_leave_race(&srv).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_quota_race() {
    let Some(url) = redis_url() else {
        eprintln!("skipping redis_quota_race");
        return;
    };
    let prefix = format!("sim-quota-{}", uuid::Uuid::new_v4());
    scenario_quota_race(redis_cfg(&url, &prefix)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_soak() {
    let Some(srv) = spawn_redis().await else {
        eprintln!("skipping redis_soak");
        return;
    };
    scenario_soak(&srv).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_throughput() {
    let Some(srv) = spawn_redis().await else {
        eprintln!("skipping redis_throughput");
        return;
    };
    scenario_throughput(&srv, "redis").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_two_node_broadcast_and_occupancy() {
    let Some((a, b)) = spawn_redis_pair().await else {
        eprintln!("skipping redis_two_node_broadcast_and_occupancy");
        return;
    };

    let mut ca = Client::connect(&a.ws).await;
    ca.subscribe_public("public-cluster").await;
    let mut cb = Client::connect(&b.ws).await;
    cb.subscribe_public("public-cluster").await;

    assert_eq!(
        trigger(&a.http, "x-node", "public-cluster", r#"{"via":"a"}"#).await,
        200
    );
    let ev = recv_event(&mut cb.ws, "x-node").await;
    assert_eq!(ev["channel"], "public-cluster");
    let ev_a = recv_event(&mut ca.ws, "x-node").await;
    assert_eq!(ev_a["channel"], "public-cluster");

    // GET counts are heartbeat snapshots (5s), not the pub/sub hot path.
    let deadline = Instant::now() + Duration::from_secs(7);
    loop {
        let info_a = channel_info(&a.http, "public-cluster", "subscription_count").await;
        let info_b = channel_info(&b.http, "public-cluster", "subscription_count").await;
        if info_a["subscription_count"] == 2 && info_b["subscription_count"] == 2 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("cluster GET count never reached 2: a={info_a} b={info_b}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_two_node_occupancy_webhook_once() {
    let Some(url) = redis_url() else {
        eprintln!("skipping redis_two_node_occupancy_webhook_once");
        return;
    };
    let (hook_url, events) = spawn_webhook_sink().await;
    let prefix = format!("sim-occ-{}", uuid::Uuid::new_v4());
    let mut cfg_a = redis_cluster_cfg(&url, &prefix);
    let mut cfg_b = redis_cluster_cfg(&url, &prefix);
    cfg_a.apps[0].webhook_url = Some(hook_url.clone());
    cfg_a.apps[0].webhook_batch_ms = 40;
    cfg_b.apps[0].webhook_url = Some(hook_url);
    cfg_b.apps[0].webhook_batch_ms = 40;
    let a = spawn_server(cfg_a).await;
    let b = spawn_server(cfg_b).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let mut ca = Client::connect(&a.ws).await;
    ca.subscribe_public("public-occ").await;
    wait_hooks(&events, "channel_occupied", "public-occ", 1).await;

    let mut cb = Client::connect(&b.ws).await;
    cb.subscribe_public("public-occ").await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        drain_named(&events, "channel_occupied", "public-occ"),
        1,
        "second node must not re-fire occupied"
    );

    // Browser-style disconnect (no unsubscribe ack). The remaining node must
    // keep occupancy; the last close should emit a single vacated.
    ca.ws.close(None).await.ok();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        drain_named(&events, "channel_vacated", "public-occ"),
        0,
        "must not vacate while the other node still has a subscriber; hooks={:?}",
        events.lock().unwrap()
    );

    cb.ws.close(None).await.ok();

    // Vacate RPC is fail-closed: timeout does not DEL. The occupancy key then
    // expires at 15s because no node still has local subscribers to EXPIRE it.
    let occ_key = format!("{prefix}:occupied:{APP_ID}:public-occ");
    let deadline = Instant::now() + Duration::from_secs(18);
    loop {
        let vacated_hooks = drain_named(&events, "channel_vacated", "public-occ");
        let redis_occ = occupancy_key(&occ_key).await;
        if vacated_hooks >= 1 || redis_occ.is_none() {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "occupancy did not clear (webhook or 15s TTL). hooks={:?} redis_occ={redis_occ:?}",
                events.lock().unwrap()
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(drain_named(&events, "channel_occupied", "public-occ"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_two_node_vacate_under_fanout() {
    let Some((responder, requester)) = spawn_redis_pair().await else {
        eprintln!("skipping redis_two_node_vacate_under_fanout");
        return;
    };

    let mut r_clients = Vec::new();
    for _ in 0..64 {
        let mut c = Client::connect(&responder.ws).await;
        c.subscribe_public("public-hol").await;
        r_clients.push(c);
    }
    let mut q = Client::connect(&requester.ws).await;
    q.subscribe_public("public-hol").await;

    let flood_http = requester.http.clone();
    let flood = tokio::spawn(async move {
        let payload = "x".repeat(4096);
        for _ in 0..30 {
            let _ = trigger(&flood_http, "hol", "public-hol", &payload).await;
        }
    });

    let incomplete_before = requester
        .state
        .metrics
        .vacate_rpc_incomplete
        .load(std::sync::atomic::Ordering::Relaxed);

    tokio::time::sleep(Duration::from_millis(30)).await;
    q.ws.close(None).await.ok();

    // Longer than redis_cluster_cfg request_timeout (1500ms) so a timeout would
    // have incremented vacate_rpc_incomplete.
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let occ_key = format!(
        "{}:occupied:{}:public-hol",
        responder.state.config.redis.prefix, APP_ID
    );
    let redis_occ = occupancy_key(&occ_key).await;
    assert!(
        redis_occ.is_some(),
        "occupancy must remain while responder still has 64 sockets; redis_occ={redis_occ:?}"
    );
    assert_eq!(
        requester
            .state
            .metrics
            .vacate_rpc_incomplete
            .load(std::sync::atomic::Ordering::Relaxed),
        incomplete_before,
        "requester last-leave RPC must complete under fan-out (received == expected)"
    );
    assert_eq!(r_clients.len(), 64);

    let _ = flood.await;
    for mut c in r_clients {
        c.ws.close(None).await.ok();
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let redis_occ = occupancy_key(&occ_key).await;
        if redis_occ.is_none() {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "occupancy did not clear after last responder leave within 5s (not 15s TTL). redis_occ={redis_occ:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn occupancy_key(key: &str) -> Option<String> {
    let client = redis::Client::open(redis_url().expect("test Redis URL")).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("GET")
        .arg(key)
        .query_async(&mut conn)
        .await
        .unwrap()
}

/// 5000-connection scale. Skipped in default `cargo test` (listen backlog + RAM).
///   cargo test --test sim local_5k -- --ignored --nocapture
///   SIM_CONNECTIONS=1000 cargo test --test sim local_5k -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "heavy: 5000 WS connections; run with --ignored"]
async fn local_5k_connections() {
    let n = scale_n();
    scenario_scale(&spawn_local().await, n, "local").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "heavy: 5000 WS connections; run with --ignored"]
async fn redis_5k_connections() {
    let n = scale_n();
    let Some(srv) = spawn_redis().await else {
        eprintln!("skipping redis_5k_connections (no Redis)");
        return;
    };
    scenario_scale(&srv, n, "redis").await;
}
