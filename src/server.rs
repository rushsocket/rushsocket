use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{info, warn};

use crate::adapter::local::LocalAdapter;
use crate::adapter::redis::RedisAdapter;
use crate::app::static_manager::StaticAppManager;
use crate::cache::CacheDriver;
use crate::cache::memory::MemoryCache;
use crate::cache::redis::RedisCache;
use crate::config::Config;
use crate::http;
use crate::state::AppState;
use crate::websocket;

/// Build the application router and shared state from config.
/// Extracted so integration tests can reuse this without the bind/serve logic.
pub async fn build_app(config: Config) -> anyhow::Result<(Router, Arc<AppState>)> {
    let app_manager = Arc::new(StaticAppManager::new(config.apps.clone()));

    let state = match config.adapter.as_str() {
        "redis" => {
            info!("Using Redis adapter ({})", config.redis.url);
            let adapter = Arc::new(
                RedisAdapter::new(&config.redis)
                    .await
                    .expect("Failed to connect to Redis"),
            );
            adapter.start_listening().await;
            let cache: Arc<dyn CacheDriver> = Arc::new(
                RedisCache::new(&config.redis.url)
                    .expect("Failed to create Redis cache pool"),
            );
            info!("Using Redis cache");
            let state = Arc::new(AppState::new_with_cache(config, app_manager, adapter.clone(), cache));
            adapter.set_metrics(state.metrics.clone());
            state
        }
        _ => {
            info!("Using local adapter");
            let adapter = Arc::new(LocalAdapter::new());
            let cache: Arc<dyn CacheDriver> = Arc::new(MemoryCache::new());
            info!("Using memory cache");
            Arc::new(AppState::new_with_cache(config, app_manager, adapter, cache))
        }
    };

    // Build the main router: WS + HTTP API
    let ws_route = Router::new()
        .route("/app/{key}", get(websocket::ws_handler))
        .with_state(state.clone());

    let http_router = http::create_router(state.clone());

    let app = ws_route.merge(http_router);

    Ok((app, state))
}

pub async fn run(config: Config) -> anyhow::Result<()> {
    // Parse bind address first — fail fast before any subsystem initialization.
    // Host must be an IP literal, not a hostname.
    let host: IpAddr = config.host.parse().map_err(|e| {
        anyhow::anyhow!(
            "Invalid host '{}': {}. Use an IP literal (e.g. 0.0.0.0, 127.0.0.1), not a hostname.",
            config.host, e
        )
    })?;
    let addr = SocketAddr::from((host, config.port));

    let (app, state) = build_app(config.clone()).await?;

    // Metrics server (separate port, always plain HTTP)
    let metrics_state = state.clone();
    let metrics_enabled = config.metrics_enabled;
    let metrics_port = config.metrics_port;

    if metrics_enabled {
        // Spawn background task to compute stats every 2 seconds
        let stats_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                interval.tick().await;
                if stats_state.is_closing() {
                    break;
                }
                let snapshot = compute_stats(&stats_state).await;
                let _ = stats_state.cached_stats_tx.send(snapshot);
            }
        });

        let metrics_router = Router::new()
            .route("/stats", get(stats_handler))
            .route("/health", get(health_handler))
            .with_state(metrics_state);

        let metrics_addr = SocketAddr::from((host, metrics_port));
        let metrics_listener = TcpListener::bind(metrics_addr).await?;
        info!("Stats server listening on {}", metrics_addr);

        tokio::spawn(async move {
            axum::serve(metrics_listener, metrics_router)
                .await
                .unwrap_or_else(|e| warn!("Metrics server error: {}", e));
        });
    }

    // If memory threshold is configured but metrics are disabled, spawn a lightweight
    // memory sampler so the admission check has fresh data.
    if !metrics_enabled && config.memory_threshold_percent > 0.0 {
        let mem_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                interval.tick().await;
                if mem_state.is_closing() {
                    break;
                }
                if let Some(pct) = tokio::task::spawn_blocking(crate::util::memory_usage_percent)
                    .await
                    .unwrap_or(None)
                {
                    mem_state.set_memory_percent(pct);
                }
            }
        });
    }

    // Two-phase shutdown: drain WS connections, then stop HTTP server.
    // Both TLS and non-TLS use axum_server::Handle for bounded graceful shutdown.
    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();
    let shutdown_state = state.clone();
    let drain_period = config.shutdown_drain_period;
    let grace_period = config.shutdown_grace_period;

    tokio::spawn(async move {
        wait_for_signal().await;
        info!("Shutdown signal received, starting graceful shutdown...");

        // Phase 1: Stop accepting new WS connections
        shutdown_state.set_closing();
        info!("Drain period: waiting {}s for WebSocket connections to drain", drain_period);
        tokio::time::sleep(std::time::Duration::from_secs(drain_period)).await;

        // Phase 2: Stop accepting new HTTP connections, give in-flight requests grace_period
        info!("Grace period: stopping HTTP server with {}s timeout", grace_period);
        shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(grace_period)));
    });

    if config.ssl.enabled {
        let cert_path = config.ssl.cert_path.as_deref()
            .expect("ssl.cert_path is required when ssl.enabled = true");
        let key_path = config.ssl.key_path.as_deref()
            .expect("ssl.key_path is required when ssl.enabled = true");

        let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            cert_path,
            key_path,
        )
        .await?;

        info!("rushsocket listening on {} (TLS)", addr);
        notify_ready();

        axum_server::bind_rustls(addr, tls_config)
            .handle(handle)
            .serve(app.into_make_service())
            .await?;
    } else {
        info!("rushsocket listening on {}", addr);
        notify_ready();

        axum_server::bind(addr)
            .handle(handle)
            .serve(app.into_make_service())
            .await?;
    }

    info!("rushsocket shut down gracefully");
    Ok(())
}

async fn wait_for_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to listen for ctrl+c");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to listen for SIGTERM")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn stats_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(state.cached_stats.borrow().clone())
}

async fn compute_stats(state: &Arc<AppState>) -> serde_json::Value {
    let uptime_secs = state.started_at.elapsed().as_secs();

    // Collect per-app live data from adapter
    let mut apps_detail = serde_json::Map::new();
    let app_configs = &state.config.apps;
    let mut total_channels: usize = 0;

    for app_cfg in app_configs {
        let app_id = &app_cfg.id;
        let channels = state.adapter.get_local_channels(app_id).await;
        let channel_count = channels.len();
        let connection_count = state.adapter.get_local_connection_count(app_id).await;
        total_channels += channel_count;

        // Per-app metrics from atomic counters
        let app_metrics = state.metrics.per_app.get(app_id)
            .map(|m| m.to_json())
            .unwrap_or_else(|| serde_json::json!({}));

        let mut app_obj = match app_metrics {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        app_obj.insert("channels".to_string(), serde_json::json!(channel_count));
        app_obj.insert("live_connections".to_string(), serde_json::json!(connection_count));

        apps_detail.insert(app_id.clone(), serde_json::Value::Object(app_obj));
    }

    // Process + system info + memory % (blocking /proc reads offloaded from async runtime)
    let (process, system, memory_pct) = tokio::task::spawn_blocking(|| {
        (read_process_info(), read_system_info(), crate::util::memory_usage_percent())
    })
    .await
    .unwrap_or_else(|_| (serde_json::json!({}), serde_json::json!({}), None));

    // Update cached memory usage for admission checks
    if let Some(pct) = memory_pct {
        state.set_memory_percent(pct);
    }

    // Network totals from metrics
    let metrics = &state.metrics;
    let ws_bytes_received = metrics.ws_bytes_received.load(std::sync::atomic::Ordering::Relaxed);
    let ws_bytes_sent = metrics.ws_bytes_sent.load(std::sync::atomic::Ordering::Relaxed);
    let http_bytes_received = metrics.http_bytes_received.load(std::sync::atomic::Ordering::Relaxed);
    let http_bytes_sent = metrics.http_bytes_sent.load(std::sync::atomic::Ordering::Relaxed);

    serde_json::json!({
        "server": {
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_seconds": uptime_secs,
            "adapter": state.config.adapter,
            "mode": state.config.mode,
            "closing": state.is_closing(),
        },
        "connections": {
            "current": metrics.connected.load(std::sync::atomic::Ordering::Relaxed),
            "total": metrics.total_connections.load(std::sync::atomic::Ordering::Relaxed),
            "total_disconnections": metrics.total_disconnections.load(std::sync::atomic::Ordering::Relaxed),
        },
        "channels": {
            "total": total_channels,
        },
        "messages": {
            "ws_received": metrics.ws_messages_received.load(std::sync::atomic::Ordering::Relaxed),
            "ws_sent": metrics.ws_messages_sent.load(std::sync::atomic::Ordering::Relaxed),
            "http_calls": metrics.http_calls.load(std::sync::atomic::Ordering::Relaxed),
        },
        "network": {
            "ws_bytes_received": ws_bytes_received,
            "ws_bytes_sent": ws_bytes_sent,
            "http_bytes_received": http_bytes_received,
            "http_bytes_sent": http_bytes_sent,
            "total_bytes_in": ws_bytes_received + http_bytes_received,
            "total_bytes_out": ws_bytes_sent + http_bytes_sent,
        },
        "process": process,
        "system": system,
        "apps": apps_detail,
    })
}

async fn health_handler() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({"status": "ok"}))
}

fn read_process_info() -> serde_json::Value {
    let pid = std::process::id();

    #[cfg(target_os = "linux")]
    {
        let mut info = serde_json::json!({
            "pid": pid,
        });

        // Memory from /proc/self/status
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            let map = info.as_object_mut().unwrap();
            for line in status.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 2 { continue; }
                match parts[0] {
                    "VmRSS:" => {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            map.insert("memory_rss_bytes".to_string(), serde_json::json!(kb * 1024));
                        }
                    }
                    "VmSize:" => {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            map.insert("memory_virtual_bytes".to_string(), serde_json::json!(kb * 1024));
                        }
                    }
                    "VmPeak:" => {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            map.insert("memory_peak_bytes".to_string(), serde_json::json!(kb * 1024));
                        }
                    }
                    "Threads:" => {
                        if let Ok(n) = parts[1].parse::<u64>() {
                            map.insert("threads".to_string(), serde_json::json!(n));
                        }
                    }
                    "voluntary_ctxt_switches:" => {
                        if let Ok(n) = parts[1].parse::<u64>() {
                            map.insert("voluntary_ctx_switches".to_string(), serde_json::json!(n));
                        }
                    }
                    "nonvoluntary_ctxt_switches:" => {
                        if let Ok(n) = parts[1].parse::<u64>() {
                            map.insert("nonvoluntary_ctx_switches".to_string(), serde_json::json!(n));
                        }
                    }
                    _ => {}
                }
            }
        }

        // CPU time from /proc/self/stat
        if let Ok(stat) = std::fs::read_to_string("/proc/self/stat") {
            // Fields: pid (comm) state ppid ... utime(14) stime(15) ...
            // Find closing ')' to skip comm field, then split remaining
            if let Some(close_paren) = stat.rfind(')') {
                let rest = &stat[close_paren + 2..]; // skip ") "
                let fields: Vec<&str> = rest.split_whitespace().collect();
                // utime is field index 11 (0-indexed from after state), stime is 12
                // state=0, ppid=1, pgrp=2, session=3, tty_nr=4, tpgid=5, flags=6,
                // minflt=7, cminflt=8, majflt=9, cmajflt=10, utime=11, stime=12
                let ticks_per_sec = 100u64; // sysconf(_SC_CLK_TCK), almost always 100
                let map = info.as_object_mut().unwrap();
                if fields.len() > 12 {
                    if let (Ok(utime), Ok(stime)) = (
                        fields[11].parse::<u64>(),
                        fields[12].parse::<u64>(),
                    ) {
                        let cpu_user_ms = (utime * 1000) / ticks_per_sec;
                        let cpu_system_ms = (stime * 1000) / ticks_per_sec;
                        map.insert("cpu_user_ms".to_string(), serde_json::json!(cpu_user_ms));
                        map.insert("cpu_system_ms".to_string(), serde_json::json!(cpu_system_ms));
                        map.insert("cpu_total_ms".to_string(), serde_json::json!(cpu_user_ms + cpu_system_ms));
                    }
                }
                // Open file descriptors: count /proc/self/fd entries
                if let Ok(fds) = std::fs::read_dir("/proc/self/fd") {
                    map.insert("open_fds".to_string(), serde_json::json!(fds.count()));
                }
            }
        }

        info
    }

    #[cfg(target_os = "windows")]
    {
        use std::mem;

        #[repr(C)]
        #[allow(non_snake_case)]
        struct ProcessMemoryCounters {
            cb: u32,
            PageFaultCount: u32,
            PeakWorkingSetSize: usize,
            WorkingSetSize: usize,
            QuotaPeakPagedPoolUsage: usize,
            QuotaPagedPoolUsage: usize,
            QuotaPeakNonPagedPoolUsage: usize,
            QuotaNonPagedPoolUsage: usize,
            PagefileUsage: usize,
            PeakPagefileUsage: usize,
        }

        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn K32GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                pmc: *mut ProcessMemoryCounters,
                cb: u32,
            ) -> i32;
        }

        let mut memory_rss: u64 = 0;
        let mut memory_peak: u64 = 0;

        unsafe {
            let mut pmc: ProcessMemoryCounters = mem::zeroed();
            pmc.cb = mem::size_of::<ProcessMemoryCounters>() as u32;
            if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) != 0 {
                memory_rss = pmc.WorkingSetSize as u64;
                memory_peak = pmc.PeakWorkingSetSize as u64;
            }
        }

        serde_json::json!({
            "pid": pid,
            "memory_rss_bytes": memory_rss,
            "memory_peak_bytes": memory_peak,
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    serde_json::json!({ "pid": pid })
}

/// Notify systemd that the service is ready (no-op on non-Linux).
#[cfg(target_os = "linux")]
fn notify_ready() {
    let _ = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]);
}

#[cfg(not(target_os = "linux"))]
fn notify_ready() {}

fn read_system_info() -> serde_json::Value {
    #[cfg(target_os = "linux")]
    {
        let mut info = serde_json::Map::new();

        // CPU count
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            let cores = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
            info.insert("cpu_cores".to_string(), serde_json::json!(cores));
        }

        // Load average from /proc/loadavg
        if let Ok(loadavg) = std::fs::read_to_string("/proc/loadavg") {
            let parts: Vec<&str> = loadavg.split_whitespace().collect();
            if parts.len() >= 3 {
                if let (Ok(l1), Ok(l5), Ok(l15)) = (
                    parts[0].parse::<f64>(),
                    parts[1].parse::<f64>(),
                    parts[2].parse::<f64>(),
                ) {
                    info.insert("load_avg_1m".to_string(), serde_json::json!(l1));
                    info.insert("load_avg_5m".to_string(), serde_json::json!(l5));
                    info.insert("load_avg_15m".to_string(), serde_json::json!(l15));
                }
            }
        }

        // Total/available memory from /proc/meminfo
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 2 { continue; }
                match parts[0] {
                    "MemTotal:" => {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            info.insert("memory_total_bytes".to_string(), serde_json::json!(kb * 1024));
                        }
                    }
                    "MemAvailable:" => {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            info.insert("memory_available_bytes".to_string(), serde_json::json!(kb * 1024));
                        }
                    }
                    _ => {}
                }
            }
        }

        // System uptime from /proc/uptime
        if let Ok(uptime) = std::fs::read_to_string("/proc/uptime") {
            if let Some(secs_str) = uptime.split_whitespace().next() {
                if let Ok(secs) = secs_str.parse::<f64>() {
                    info.insert("uptime_seconds".to_string(), serde_json::json!(secs as u64));
                }
            }
        }

        serde_json::Value::Object(info)
    }

    #[cfg(not(target_os = "linux"))]
    serde_json::json!({})
}
