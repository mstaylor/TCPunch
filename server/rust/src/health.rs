//! Health check HTTP endpoint for load balancer integration

use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;

use crate::registry::Registry;
use crate::server::{MetricsSnapshot, ServerMetrics};

/// Health check state
pub struct HealthState {
    pub metrics: Arc<ServerMetrics>,
    pub registry: Registry,
    pub start_time: Instant,
    pub server_id: String,
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub server_id: String,
    pub uptime_secs: u64,
    pub active_pairings: usize,
    pub metrics: MetricsSnapshot,
}

/// Liveness probe - just checks if server is running
async fn liveness() -> impl IntoResponse {
    StatusCode::OK
}

/// Readiness probe - checks if server can accept connections
async fn readiness(State(state): State<Arc<HealthState>>) -> impl IntoResponse {
    let active_pairings = state.registry.active_count().await;
    let metrics = state.metrics.snapshot();

    let response = HealthResponse {
        status: "ready",
        server_id: state.server_id.clone(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        active_pairings,
        metrics,
    };

    (StatusCode::OK, Json(response))
}

/// Detailed health check with metrics
async fn health(State(state): State<Arc<HealthState>>) -> impl IntoResponse {
    let active_pairings = state.registry.active_count().await;
    let metrics = state.metrics.snapshot();

    let response = HealthResponse {
        status: "healthy",
        server_id: state.server_id.clone(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        active_pairings,
        metrics,
    };

    (StatusCode::OK, Json(response))
}

/// Prometheus-compatible metrics endpoint
async fn prometheus_metrics(State(state): State<Arc<HealthState>>) -> impl IntoResponse {
    let metrics = state.metrics.snapshot();
    let active_pairings = state.registry.active_count().await;

    let output = format!(
        r#"# HELP tcpunch_active_connections Current number of active connections
# TYPE tcpunch_active_connections gauge
tcpunch_active_connections {{server_id="{server_id}"}} {active_connections}

# HELP tcpunch_total_connections Total number of connections since start
# TYPE tcpunch_total_connections counter
tcpunch_total_connections {{server_id="{server_id}"}} {total_connections}

# HELP tcpunch_successful_pairings Total successful pairings
# TYPE tcpunch_successful_pairings counter
tcpunch_successful_pairings {{server_id="{server_id}"}} {successful_pairings}

# HELP tcpunch_timeouts Total timeout events
# TYPE tcpunch_timeouts counter
tcpunch_timeouts {{server_id="{server_id}"}} {timeouts}

# HELP tcpunch_errors Total error events
# TYPE tcpunch_errors counter
tcpunch_errors {{server_id="{server_id}"}} {errors}

# HELP tcpunch_active_pairings Current number of active pairing entries
# TYPE tcpunch_active_pairings gauge
tcpunch_active_pairings {{server_id="{server_id}"}} {active_pairings}

# HELP tcpunch_uptime_seconds Server uptime in seconds
# TYPE tcpunch_uptime_seconds gauge
tcpunch_uptime_seconds {{server_id="{server_id}"}} {uptime}
"#,
        server_id = state.server_id,
        active_connections = metrics.active_connections,
        total_connections = metrics.total_connections,
        successful_pairings = metrics.successful_pairings,
        timeouts = metrics.timeouts,
        errors = metrics.errors,
        active_pairings = active_pairings,
        uptime = state.start_time.elapsed().as_secs(),
    );

    (
        StatusCode::OK,
        [("content-type", "text/plain; charset=utf-8")],
        output,
    )
}

/// Create the health check router
pub fn health_router(state: Arc<HealthState>) -> Router {
    Router::new()
        .route("/livez", get(liveness))
        .route("/readyz", get(readiness))
        .route("/health", get(health))
        .route("/metrics", get(prometheus_metrics))
        .with_state(state)
}

/// Run the health check HTTP server
pub async fn run_health_server(
    state: Arc<HealthState>,
    port: u16,
) -> anyhow::Result<()> {
    let app = health_router(state);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));

    tracing::info!(%addr, "Health check server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
