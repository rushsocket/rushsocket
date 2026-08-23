#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use rushsocket::config::Config;

#[derive(Parser, Debug)]
#[command(
    name = "rushsocket",
    about = "Pusher Protocol v7 compatible WebSocket server"
)]
struct Cli {
    /// Path to config file
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load config
    let mut config = if let Some(path) = &cli.config {
        Config::load(path).unwrap_or_else(|e| {
            eprintln!("Error: Could not load config file '{}': {}", path, e);
            std::process::exit(1);
        })
    } else {
        Config::default()
    };

    // Init logging
    let filter = if config.debug {
        EnvFilter::new("rushsocket=debug,tower_http=debug")
    } else {
        EnvFilter::new("rushsocket=info")
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!(
        "Starting rushsocket v{} on {}:{}",
        env!("CARGO_PKG_VERSION"),
        config.host,
        config.port
    );

    if config.apps.is_empty() {
        if cli.config.is_some() {
            eprintln!("Error: config file has no [[apps]] entries");
            std::process::exit(1);
        }
        info!("No apps configured, using default app on loopback (secret not logged)");
        config.apps.push(rushsocket::config::AppConfig::default());
    }
    if let Err(e) = config.validate() {
        eprintln!("Error: invalid config: {e}");
        std::process::exit(1);
    }

    for app in &config.apps {
        info!("App registered: id={}, key={}", app.id, app.key);
    }

    rushsocket::server::run(config).await
}
