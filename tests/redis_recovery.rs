//! Exercises recovery with a disposable Redis server. Never restarts the configured Redis.
use std::io::ErrorKind;
use std::net::TcpListener;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use rushsocket::adapter::redis::RedisAdapter;
use rushsocket::adapter::{Adapter, Admit};
use rushsocket::app::App;
use rushsocket::config::{AppConfig, RedisConfig};
use rushsocket::metrics::Metrics;
use rushsocket::websocket::connection::WsConnection;
use serde_json::json;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

const APP: &str = "recovery";
const CHANNEL: &str = "presence-room";

async fn ready(adapter: &RedisAdapter, expected: bool) {
    timeout(Duration::from_secs(15), async {
        while adapter.is_ready() != expected {
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("adapter readiness transition");
}

async fn adapter(cfg: &RedisConfig) -> anyhow::Result<Arc<RedisAdapter>> {
    Ok(Arc::new(
        RedisAdapter::new(cfg, Arc::new(Metrics::new())).await?,
    ))
}

fn socket(id: &str) -> Arc<WsConnection> {
    let app = App::from(AppConfig {
        id: APP.into(),
        ..Default::default()
    });
    Arc::new(WsConnection::new(
        id.into(),
        Arc::new(app),
        tokio::sync::mpsc::channel(4).0,
    ))
}

async fn start_redis(cmd: &mut Command, port: u16) -> std::io::Result<Child> {
    let mut child = cmd.spawn()?;
    timeout(Duration::from_secs(5), async {
        while tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            if let Some(status) = child.try_wait()? {
                return Err(std::io::Error::other(format!("Redis exited: {status}")));
            }
            sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    })
    .await??;
    Ok(child)
}

#[tokio::test]
async fn redis_restart_peer_acknowledgements_and_presence_leases() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let mut cmd = Command::new("redis-server");
    cmd.args([
        "--bind",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--appendonly",
        "no",
        "--save",
        "",
    ])
    .current_dir(dir.path())
    .stdout(Stdio::null())
    .kill_on_drop(true);
    let mut db = match start_redis(&mut cmd, port).await {
        Err(e) if e.kind() == ErrorKind::NotFound && std::env::var_os("CI").is_none() => {
            eprintln!("skipping isolated Redis restart test (redis-server unavailable)");
            return Ok(());
        }
        result => result?,
    };
    let cfg = RedisConfig {
        url: format!("redis://127.0.0.1:{port}"),
        request_timeout: 1000,
        ..Default::default()
    };
    let a = adapter(&cfg).await?;
    let b = adapter(&cfg).await?;
    for node in [&a, &b] {
        node.start_listening().await;
        ready(node, true).await;
    }

    let terminated = socket("9.9");
    assert_eq!(
        b.try_add_socket(APP, terminated.clone(), 2).await,
        Admit::Ok
    );
    b.add_user(APP, "terminated", "9.9").await;
    a.try_terminate_user_connections(APP, "terminated").await?;
    assert!(terminated.should_close());
    b.remove_user(APP, "terminated", "9.9").await;
    b.remove_socket(APP, "9.9").await;

    let user = socket("1.1");
    assert_eq!(b.try_add_socket(APP, user.clone(), 2).await, Admit::Ok);

    b.add_to_channel(APP, "1.1", CHANNEL).await;
    let info = json!({"roles": [], "user_number": "001"});
    b.try_add_presence_member(APP, CHANNEL, "1.1", "alice", &info, 2)
        .await
        .unwrap();

    // A subscriber which cannot confirm termination must prevent a success response.
    let mut silent = redis::Client::open(cfg.url.as_str())?
        .get_async_pubsub()
        .await?;
    silent.subscribe(format!("{}#request", cfg.prefix)).await?;
    assert!(
        a.try_terminate_user_connections(APP, "absent")
            .await
            .is_err()
    );
    drop(silent);

    let signed_in = socket("6.6");
    assert_eq!(b.try_add_socket(APP, signed_in.clone(), 2).await, Admit::Ok);
    b.add_user(APP, "signed-in", "6.6").await;
    db.kill().await?;
    ready(&a, false).await;
    ready(&b, false).await;
    timeout(Duration::from_secs(1), signed_in.closed()).await?;
    assert!(!user.should_close());
    let _db = start_redis(&mut cmd, port).await?;
    ready(&a, true).await;
    ready(&b, true).await;
    b.remove_user(APP, "signed-in", "6.6").await;
    b.remove_socket(APP, "6.6").await;
    assert_eq!(a.get_presence_members(APP, CHANNEL).await["alice"], info);
    assert_eq!(a.try_add_socket(APP, socket("2.2"), 2).await, Admit::Ok);
    assert_eq!(
        a.try_add_socket(APP, socket("3.3"), 2).await,
        Admit::OverQuota
    );

    // This owner never runs a heartbeat or a graceful disconnect.
    let lost = adapter(&cfg).await?;
    lost.try_add_presence_member(APP, CHANNEL, "4.4", "bob", &json!(null), 2)
        .await
        .unwrap();
    let (new_alice, _) = lost
        .try_add_presence_member(APP, CHANNEL, "5.5", "alice", &info, 2)
        .await
        .unwrap();
    assert!(!new_alice);
    drop(lost);
    assert!(
        a.try_add_presence_member(APP, CHANNEL, "2.2", "carol", &info, 2)
            .await
            .is_err()
    );
    timeout(Duration::from_secs(26), async {
        while a.get_presence_user_count(APP, CHANNEL).await != 1 {
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    assert_eq!(a.get_presence_members(APP, CHANNEL).await["alice"], info);
    assert!(
        a.try_add_presence_member(APP, CHANNEL, "2.2", "carol", &info, 2)
            .await
            .is_ok()
    );
    Ok(())
}
