//! TCPunch Rendezvous Server
//!
//! A TCP hole punching rendezvous server that facilitates P2P connections
//! between clients behind NAT/firewalls.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use tokio::sync::watch;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod health;
mod protocol;
mod registry;
mod server;

pub use protocol::*;
pub use registry::*;
pub use server::*;

/// TCPunch Rendezvous Server
#[derive(Parser, Debug)]
#[command(name = "tcpunchd")]
#[command(about = "TCP hole punching rendezvous server")]
struct Args {
    /// Port to listen on for client connections
    #[arg(short, long, default_value_t = 10000, env = "TCPUNCH_PORT")]
    port: u16,

    /// Port for health check HTTP endpoint
    #[arg(long, default_value_t = 10001, env = "TCPUNCH_HEALTH_PORT")]
    health_port: u16,

    /// Redis URL for multi-server mode (optional)
    #[arg(long, env = "REDIS_URL")]
    redis_url: Option<String>,

    /// Timeout for receiving client request (seconds)
    #[arg(long, default_value_t = 5, env = "TCPUNCH_RECV_TIMEOUT")]
    recv_timeout: u64,

    /// Timeout for waiting for peer (seconds)
    #[arg(long, default_value_t = 60, env = "TCPUNCH_PEER_TIMEOUT")]
    peer_timeout: u64,

    /// Maximum concurrent connections
    #[arg(long, default_value_t = 10000, env = "TCPUNCH_MAX_CONNECTIONS")]
    max_connections: usize,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info", env = "RUST_LOG")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| args.log_level.clone().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting TCPunch server");
    tracing::info!(port = args.port, health_port = args.health_port, "Configuration");

    // Generate server ID
    let server_id = uuid::Uuid::new_v4().to_string();
    tracing::info!(%server_id, "Server ID");

    // Create registry configuration
    let registry_config = registry::RegistryConfig {
        pairing_timeout: Duration::from_secs(args.peer_timeout),
        cleanup_interval: Duration::from_secs(30),
        server_id: server_id.clone(),
    };

    // Create registry (Redis or in-memory)
    let registry = if let Some(redis_url) = &args.redis_url {
        tracing::info!(%redis_url, "Using Redis backend");
        let redis_registry = registry::RedisRegistry::new(registry_config, redis_url).await?;
        let redis_registry = Arc::new(redis_registry);

        // Start background tasks
        redis_registry.clone().start_background_tasks();

        registry::Registry::Redis(redis_registry)
    } else {
        tracing::info!("Using in-memory backend (single-server mode)");
        let memory_registry = registry::InMemoryRegistry::new(registry_config);
        let memory_registry = Arc::new(memory_registry);

        // Start cleanup task
        memory_registry.clone().start_cleanup_task();

        registry::Registry::InMemory(memory_registry)
    };

    // Create server configuration
    let server_config = server::ServerConfig {
        listen_port: args.port,
        recv_timeout: Duration::from_secs(args.recv_timeout),
        peer_wait_timeout: Duration::from_secs(args.peer_timeout),
        max_connections: args.max_connections,
    };

    // Create shared metrics
    let metrics = Arc::new(server::ServerMetrics::new());

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create server state
    let server_state = Arc::new(server::ServerState {
        config: server_config,
        registry: registry.clone(),
        metrics: Arc::clone(&metrics),
        shutdown_rx: shutdown_rx.clone(),
    });

    // Create health check state
    let health_state = Arc::new(health::HealthState {
        metrics: Arc::clone(&metrics),
        registry,
        start_time: Instant::now(),
        server_id,
    });

    // Spawn health check server
    let health_handle = tokio::spawn(health::run_health_server(
        health_state,
        args.health_port,
    ));

    // Spawn main TCP server
    let server_handle = tokio::spawn(server::run_server(
        Arc::clone(&server_state),
        shutdown_rx.clone(),
    ));

    // Wait for shutdown signal
    shutdown_signal().await;

    tracing::info!("Initiating graceful shutdown...");

    // Signal shutdown
    let _ = shutdown_tx.send(true);

    // Wait for drain period (let active connections complete)
    let drain_timeout = Duration::from_secs(30);
    tracing::info!(timeout_secs = drain_timeout.as_secs(), "Draining connections");

    tokio::select! {
        _ = tokio::time::sleep(drain_timeout) => {
            tracing::warn!("Drain timeout reached, forcing shutdown");
        }
        _ = async {
            loop {
                let active = metrics.active_connections.load(std::sync::atomic::Ordering::Relaxed);
                if active == 0 {
                    tracing::info!("All connections drained");
                    break;
                }
                tracing::info!(active, "Waiting for connections to drain");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        } => {}
    }

    // Abort remaining tasks
    health_handle.abort();
    server_handle.abort();

    tracing::info!("Shutdown complete");
    Ok(())
}

/// Wait for shutdown signal (SIGTERM or SIGINT)
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C");
        }
        _ = terminate => {
            tracing::info!("Received SIGTERM");
        }
    }
}
