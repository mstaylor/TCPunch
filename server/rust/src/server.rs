//! TCP server implementation for TCPunch rendezvous server

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::timeout;

use crate::protocol::{
    ClientRequest, PeerConnectionData, ServerResponse, CLIENT_REQUEST_SIZE,
};
use crate::registry::{Registry, RegistrationResult};

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Port to listen on
    pub listen_port: u16,
    /// Timeout for receiving client request
    pub recv_timeout: Duration,
    /// Timeout for waiting for peer
    pub peer_wait_timeout: Duration,
    /// Maximum concurrent connections
    pub max_connections: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_port: 10000,
            recv_timeout: Duration::from_secs(5),
            peer_wait_timeout: Duration::from_secs(60),
            max_connections: 10000,
        }
    }
}

/// Connection metrics
#[derive(Debug, Default)]
pub struct ServerMetrics {
    pub active_connections: std::sync::atomic::AtomicUsize,
    pub total_connections: std::sync::atomic::AtomicU64,
    pub successful_pairings: std::sync::atomic::AtomicU64,
    pub timeouts: std::sync::atomic::AtomicU64,
    pub errors: std::sync::atomic::AtomicU64,
}

impl ServerMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn connection_started(&self) {
        use std::sync::atomic::Ordering;
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        self.total_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_ended(&self) {
        use std::sync::atomic::Ordering;
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn pairing_succeeded(&self) {
        use std::sync::atomic::Ordering;
        self.successful_pairings.fetch_add(1, Ordering::Relaxed);
    }

    pub fn timeout_occurred(&self) {
        use std::sync::atomic::Ordering;
        self.timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn error_occurred(&self) {
        use std::sync::atomic::Ordering;
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        use std::sync::atomic::Ordering;
        MetricsSnapshot {
            active_connections: self.active_connections.load(Ordering::Relaxed),
            total_connections: self.total_connections.load(Ordering::Relaxed),
            successful_pairings: self.successful_pairings.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub active_connections: usize,
    pub total_connections: u64,
    pub successful_pairings: u64,
    pub timeouts: u64,
    pub errors: u64,
}

/// Shared server state
pub struct ServerState {
    pub config: ServerConfig,
    pub registry: Registry,
    pub metrics: Arc<ServerMetrics>,
    pub shutdown_rx: watch::Receiver<bool>,
}

/// Run the TCP server
pub async fn run_server(
    state: Arc<ServerState>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], state.config.listen_port));
    let listener = TcpListener::bind(addr).await?;

    tracing::info!(%addr, "Server listening");

    loop {
        tokio::select! {
            // Check for shutdown signal
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("Shutdown signal received, stopping accept loop");
                    break;
                }
            }

            // Accept new connections
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, peer_addr)) => {
                        let state = Arc::clone(&state);

                        // Check connection limit
                        let active = state.metrics.active_connections.load(
                            std::sync::atomic::Ordering::Relaxed
                        );
                        if active >= state.config.max_connections {
                            tracing::warn!(active, max = state.config.max_connections, "Connection limit reached");
                            drop(stream);
                            continue;
                        }

                        // Spawn handler task
                        tokio::spawn(async move {
                            state.metrics.connection_started();

                            if let Err(e) = handle_connection(stream, peer_addr, &state).await {
                                tracing::debug!(%peer_addr, error = %e, "Connection error");
                                state.metrics.error_occurred();
                            }

                            state.metrics.connection_ended();
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Accept failed");
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle a single client connection
async fn handle_connection(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    state: &ServerState,
) -> anyhow::Result<()> {
    tracing::debug!(%peer_addr, "New connection");

    // Get client's public address
    let client_info = match PeerConnectionData::from_socket_addr(peer_addr) {
        Some(info) => info,
        None => {
            tracing::warn!(%peer_addr, "IPv6 not supported");
            let resp = ServerResponse::error();
            stream.write_all(&resp.to_bytes()).await?;
            return Ok(());
        }
    };

    // Read client request with timeout
    let mut buf = [0u8; CLIENT_REQUEST_SIZE];
    let request = match timeout(state.config.recv_timeout, stream.read_exact(&mut buf)).await {
        Ok(Ok(_)) => match ClientRequest::from_bytes(&buf) {
            Ok(req) => req,
            Err(e) => {
                tracing::debug!(%peer_addr, error = %e, "Invalid request");
                let resp = ServerResponse::error();
                stream.write_all(&resp.to_bytes()).await?;
                return Ok(());
            }
        },
        Ok(Err(e)) => {
            tracing::debug!(%peer_addr, error = %e, "Read error");
            return Err(e.into());
        }
        Err(_) => {
            tracing::debug!(%peer_addr, "Request timeout");
            state.metrics.timeout_occurred();
            return Ok(());
        }
    };

    tracing::info!(
        %peer_addr,
        pairing_name = %request.pairing_name,
        has_token = request.reconnect_token.is_some(),
        "Client request"
    );

    // Register with pairing registry
    let result = state
        .registry
        .register(&request.pairing_name, client_info, request.reconnect_token)
        .await;

    match result {
        Ok(RegistrationResult::Paired { token, peer_info }) => {
            // Immediate pairing - send peer info
            tracing::info!(
                %peer_addr,
                pairing_name = %request.pairing_name,
                peer = ?peer_info,
                "Paired immediately"
            );

            let resp = ServerResponse::paired(client_info, peer_info, token);
            stream.write_all(&resp.to_bytes()).await?;
            state.metrics.pairing_succeeded();
        }

        Ok(RegistrationResult::Waiting { token, peer_notify })
        | Ok(RegistrationResult::Resumed { token, peer_notify }) => {
            // Send initial "waiting" response
            let resp = ServerResponse::waiting(client_info, token);
            stream.write_all(&resp.to_bytes()).await?;

            tracing::debug!(
                %peer_addr,
                pairing_name = %request.pairing_name,
                "Waiting for peer"
            );

            // Wait for peer with timeout
            let wait_result = timeout(state.config.peer_wait_timeout, peer_notify).await;

            match wait_result {
                Ok(Ok(peer_info)) => {
                    // Peer arrived - send their info
                    tracing::info!(
                        %peer_addr,
                        pairing_name = %request.pairing_name,
                        peer = ?peer_info,
                        "Peer arrived"
                    );

                    let resp = ServerResponse::paired(client_info, peer_info, token);
                    stream.write_all(&resp.to_bytes()).await?;
                    state.metrics.pairing_succeeded();
                }
                Ok(Err(_)) => {
                    // Sender dropped (shouldn't happen normally)
                    tracing::debug!(
                        %peer_addr,
                        pairing_name = %request.pairing_name,
                        "Peer notification channel closed"
                    );

                    let resp = ServerResponse::timeout(client_info, token);
                    stream.write_all(&resp.to_bytes()).await?;
                    state.metrics.timeout_occurred();
                }
                Err(_) => {
                    // Timeout waiting for peer
                    tracing::debug!(
                        %peer_addr,
                        pairing_name = %request.pairing_name,
                        "Timeout waiting for peer"
                    );

                    // Unregister from registry
                    let _ = state
                        .registry
                        .unregister(&request.pairing_name, token)
                        .await;

                    let resp = ServerResponse::timeout(client_info, token);
                    stream.write_all(&resp.to_bytes()).await?;
                    state.metrics.timeout_occurred();
                }
            }
        }

        Err(e) => {
            tracing::warn!(%peer_addr, error = %e, "Registration failed");
            let resp = ServerResponse::error();
            stream.write_all(&resp.to_bytes()).await?;
        }
    }

    Ok(())
}
