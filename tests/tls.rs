use std::io::Write;
use std::net::TcpListener;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::{sleep, timeout};

#[tokio::test]
async fn tls_listener_serves_ready_in_a_fresh_process() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let mut config = tempfile::Builder::new().suffix(".toml").tempfile()?;
    writeln!(
        config,
        r#"
host = "127.0.0.1"
port = {}
metrics_enabled = false
[ssl]
enabled = true
cert_path = "localhost.pem"
key_path = "localhost-key.pem"
[[apps]]
id = "tls-test"
key = "test-key"
secret = "test-secret"
"#,
        addr.port()
    )?;

    let mut command = Command::new(env!("CARGO_BIN_EXE_rushsocket"));
    command
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tls"))
        .arg("--config")
        .arg(config.path())
        .kill_on_drop(true);

    #[cfg(target_os = "linux")]
    let (_notify_dir, notify_socket) = {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notify.sock");
        let socket = tokio::net::UnixDatagram::bind(&path)?;
        command.env("NOTIFY_SOCKET", path);
        // A failed bind must exit without telling systemd the unit is ready.
        assert!(
            !timeout(Duration::from_secs(10), command.status())
                .await??
                .success()
        );
        let mut data = [0u8; 128];
        assert_eq!(
            socket.try_recv(&mut data).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        (dir, socket)
    };

    drop(listener);
    let mut server = command.spawn()?;
    #[cfg(target_os = "linux")]
    {
        let mut data = [0u8; 128];
        let size = timeout(Duration::from_secs(10), notify_socket.recv(&mut data)).await??;
        assert!(
            std::str::from_utf8(&data[..size])?
                .lines()
                .any(|line| line == "READY=1")
        );
    }
    timeout(Duration::from_secs(10), async {
        while TcpStream::connect(addr).await.is_err() {
            if let Some(status) = server.try_wait()? {
                anyhow::bail!("TLS listener exited during startup: {status}");
            }
            sleep(Duration::from_millis(20)).await;
        }
        anyhow::Ok(())
    })
    .await??;

    let cert = reqwest::Certificate::from_pem(include_bytes!("fixtures/tls/localhost.pem"))?;
    let response = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(cert)
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(format!("https://{addr}/ready"))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.text().await?, "OK");
    server.kill().await?;
    Ok(())
}
