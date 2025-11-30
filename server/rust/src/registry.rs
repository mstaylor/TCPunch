//! Pairing registry - manages client pairing state
//!
//! Supports two backends:
//! - InMemory: Single-server mode, no external dependencies
//! - Redis: Multi-server mode with failover support

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::protocol::PeerConnectionData;

/// Configuration for the pairing registry
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    /// How long a client can wait for a peer before timeout
    pub pairing_timeout: Duration,
    /// How often to run cleanup of stale entries
    pub cleanup_interval: Duration,
    /// Server identifier (for multi-server mode)
    pub server_id: String,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            pairing_timeout: Duration::from_secs(60),
            cleanup_interval: Duration::from_secs(30),
            server_id: Uuid::new_v4().to_string(),
        }
    }
}

/// A waiting client's registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitingClient {
    pub connection_info: PeerConnectionData,
    pub reconnect_token: Uuid,
    pub server_id: String,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub registered_at: chrono::DateTime<chrono::Utc>,
}

/// Internal entry in the pairing map
struct PairingEntry {
    waiting_client: WaitingClient,
    /// Channel to notify when peer arrives
    notify_tx: Option<oneshot::Sender<PeerConnectionData>>,
    created_at: Instant,
}

/// Result of attempting to register for pairing
pub enum RegistrationResult {
    /// First client for this pairing name, waiting for peer
    Waiting {
        token: Uuid,
        /// Receiver that fires when peer connects
        peer_notify: oneshot::Receiver<PeerConnectionData>,
    },
    /// Second client, pairing complete - here's the peer info
    Paired {
        token: Uuid,
        peer_info: PeerConnectionData,
    },
    /// Reconnected with valid token, resuming wait
    Resumed {
        token: Uuid,
        peer_notify: oneshot::Receiver<PeerConnectionData>,
    },
}

/// Errors from registry operations
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("Invalid reconnect token")]
    InvalidToken,

    #[error("Pairing session expired")]
    SessionExpired,

    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// In-memory pairing registry (single-server mode)
pub struct InMemoryRegistry {
    config: RegistryConfig,
    /// Map of pairing_name -> entry
    pairings: DashMap<String, PairingEntry>,
    /// Map of token -> pairing_name (for reconnection)
    tokens: DashMap<Uuid, String>,
}

impl InMemoryRegistry {
    pub fn new(config: RegistryConfig) -> Self {
        Self {
            config,
            pairings: DashMap::new(),
            tokens: DashMap::new(),
        }
    }

    /// Start background cleanup task
    pub fn start_cleanup_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let interval = self.config.cleanup_interval;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let cleaned = self.cleanup_expired();
                if cleaned > 0 {
                    tracing::info!(cleaned, "Cleaned up expired pairing entries");
                }
            }
        })
    }

    pub async fn register(
        &self,
        pairing_name: &str,
        client_info: PeerConnectionData,
        reconnect_token: Option<Uuid>,
    ) -> Result<RegistrationResult, RegistryError> {
        // Check for reconnection with existing token
        if let Some(token) = reconnect_token {
            if let Some(stored_name) = self.tokens.get(&token) {
                if stored_name.value() == pairing_name {
                    // Valid reconnection - check if entry still exists
                    if let Some(mut entry) = self.pairings.get_mut(pairing_name) {
                        if entry.waiting_client.reconnect_token == token {
                            // Update connection info (may have changed)
                            entry.waiting_client.connection_info = client_info;

                            // Create new notification channel
                            let (tx, rx) = oneshot::channel();
                            entry.notify_tx = Some(tx);

                            return Ok(RegistrationResult::Resumed {
                                token,
                                peer_notify: rx,
                            });
                        }
                    }
                }
            }
            // Invalid or expired token - continue with fresh registration
            tracing::debug!(?token, "Reconnect token invalid or expired");
        }

        // Check if there's already a waiting client for this pairing
        if let Some((_, mut entry)) = self.pairings.remove(pairing_name) {
            // Second client arrived - complete the pairing
            let peer_info = entry.waiting_client.connection_info;
            let peer_token = entry.waiting_client.reconnect_token;

            // Notify the waiting client
            if let Some(tx) = entry.notify_tx.take() {
                let _ = tx.send(client_info);
            }

            // Clean up the waiting client's token
            self.tokens.remove(&peer_token);

            // Generate token for this client (for protocol consistency)
            let token = Uuid::new_v4();

            return Ok(RegistrationResult::Paired { token, peer_info });
        }

        // First client - register and wait
        let token = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();

        let waiting_client = WaitingClient {
            connection_info: client_info,
            reconnect_token: token,
            server_id: self.config.server_id.clone(),
            registered_at: chrono::Utc::now(),
        };

        let entry = PairingEntry {
            waiting_client,
            notify_tx: Some(tx),
            created_at: Instant::now(),
        };

        self.pairings.insert(pairing_name.to_string(), entry);
        self.tokens.insert(token, pairing_name.to_string());

        Ok(RegistrationResult::Waiting {
            token,
            peer_notify: rx,
        })
    }

    pub async fn unregister(&self, pairing_name: &str, token: Uuid) -> Result<(), RegistryError> {
        if let Some((_, entry)) = self.pairings.remove(pairing_name) {
            if entry.waiting_client.reconnect_token == token {
                self.tokens.remove(&token);
            }
        }
        Ok(())
    }

    pub fn active_count(&self) -> usize {
        self.pairings.len()
    }

    pub fn cleanup_expired(&self) -> usize {
        let timeout = self.config.pairing_timeout;
        let now = Instant::now();
        let mut cleaned = 0;

        self.pairings.retain(|_, entry| {
            if now.duration_since(entry.created_at) > timeout {
                // Entry expired - remove it
                self.tokens.remove(&entry.waiting_client.reconnect_token);
                cleaned += 1;
                false
            } else {
                true
            }
        });

        cleaned
    }
}

/// Redis-backed pairing registry (multi-server mode)
pub struct RedisRegistry {
    config: RegistryConfig,
    client: redis::aio::ConnectionManager,
    /// Local notification channels (for clients connected to this server)
    local_waiters: DashMap<String, oneshot::Sender<PeerConnectionData>>,
}

impl RedisRegistry {
    pub async fn new(config: RegistryConfig, redis_url: &str) -> Result<Self, RegistryError> {
        let client = redis::Client::open(redis_url)?;
        let connection = client.get_connection_manager().await?;

        Ok(Self {
            config,
            client: connection,
            local_waiters: DashMap::new(),
        })
    }

    fn pairing_key(pairing_name: &str) -> String {
        format!("tcpunch:pairing:{}", pairing_name)
    }

    fn token_key(token: Uuid) -> String {
        format!("tcpunch:token:{}", token)
    }

    /// Start background tasks (cleanup + pub/sub listener)
    pub fn start_background_tasks(self: Arc<Self>) -> Vec<tokio::task::JoinHandle<()>> {
        let cleanup_handle = {
            let this = Arc::clone(&self);
            let interval = this.config.cleanup_interval;
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                loop {
                    ticker.tick().await;
                    let cleaned = this.cleanup_expired().await;
                    if cleaned > 0 {
                        tracing::info!(cleaned, "Cleaned up expired Redis entries");
                    }
                }
            })
        };

        vec![cleanup_handle]
    }

    pub async fn register(
        &self,
        pairing_name: &str,
        client_info: PeerConnectionData,
        reconnect_token: Option<Uuid>,
    ) -> Result<RegistrationResult, RegistryError> {
        let mut conn = self.client.clone();
        let pairing_key = Self::pairing_key(pairing_name);

        // Check for reconnection
        if let Some(token) = reconnect_token {
            let token_key = Self::token_key(token);
            let stored_name: Option<String> = redis::cmd("GET")
                .arg(&token_key)
                .query_async(&mut conn)
                .await?;

            if stored_name.as_deref() == Some(pairing_name) {
                // Valid reconnection - update the entry
                let waiting = WaitingClient {
                    connection_info: client_info,
                    reconnect_token: token,
                    server_id: self.config.server_id.clone(),
                    registered_at: chrono::Utc::now(),
                };

                let waiting_json = serde_json::to_string(&waiting)
                    .map_err(|e| RegistryError::Internal(e.to_string()))?;

                let ttl_secs = self.config.pairing_timeout.as_secs() as i64;

                let _: () = redis::cmd("SETEX")
                    .arg(&pairing_key)
                    .arg(ttl_secs)
                    .arg(&waiting_json)
                    .query_async(&mut conn)
                    .await?;

                let (tx, rx) = oneshot::channel();
                self.local_waiters.insert(pairing_name.to_string(), tx);

                return Ok(RegistrationResult::Resumed {
                    token,
                    peer_notify: rx,
                });
            }
        }

        // Try to get existing waiting client (atomic GET + DEL)
        let existing: Option<String> = redis::cmd("GETDEL")
            .arg(&pairing_key)
            .query_async(&mut conn)
            .await?;

        if let Some(waiting_json) = existing {
            // Second client - complete pairing
            let waiting: WaitingClient = serde_json::from_str(&waiting_json)
                .map_err(|e| RegistryError::Internal(e.to_string()))?;

            let peer_info = waiting.connection_info;

            // Clean up peer's token
            let peer_token_key = Self::token_key(waiting.reconnect_token);
            let _: () = redis::cmd("DEL")
                .arg(&peer_token_key)
                .query_async(&mut conn)
                .await?;

            // Notify peer if they're on this server
            if let Some((_, tx)) = self.local_waiters.remove(pairing_name) {
                let _ = tx.send(client_info);
            } else if waiting.server_id != self.config.server_id {
                // Peer is on different server - publish notification
                let notify_channel = format!("tcpunch:notify:{}", pairing_name);
                let notify_data = serde_json::to_string(&client_info)
                    .map_err(|e| RegistryError::Internal(e.to_string()))?;

                let _: () = redis::cmd("PUBLISH")
                    .arg(&notify_channel)
                    .arg(&notify_data)
                    .query_async(&mut conn)
                    .await?;
            }

            let token = Uuid::new_v4();
            return Ok(RegistrationResult::Paired { token, peer_info });
        }

        // First client - register and wait
        let token = Uuid::new_v4();
        let waiting = WaitingClient {
            connection_info: client_info,
            reconnect_token: token,
            server_id: self.config.server_id.clone(),
            registered_at: chrono::Utc::now(),
        };

        let waiting_json = serde_json::to_string(&waiting)
            .map_err(|e| RegistryError::Internal(e.to_string()))?;

        let ttl_secs = self.config.pairing_timeout.as_secs() as i64;

        // Set pairing entry with TTL
        let _: () = redis::cmd("SETEX")
            .arg(&pairing_key)
            .arg(ttl_secs)
            .arg(&waiting_json)
            .query_async(&mut conn)
            .await?;

        // Set token -> pairing_name mapping with TTL
        let token_key = Self::token_key(token);
        let _: () = redis::cmd("SETEX")
            .arg(&token_key)
            .arg(ttl_secs)
            .arg(pairing_name)
            .query_async(&mut conn)
            .await?;

        let (tx, rx) = oneshot::channel();
        self.local_waiters.insert(pairing_name.to_string(), tx);

        Ok(RegistrationResult::Waiting {
            token,
            peer_notify: rx,
        })
    }

    pub async fn unregister(&self, pairing_name: &str, token: Uuid) -> Result<(), RegistryError> {
        let mut conn = self.client.clone();

        let pairing_key = Self::pairing_key(pairing_name);
        let token_key = Self::token_key(token);

        let _: () = redis::cmd("DEL")
            .arg(&pairing_key)
            .arg(&token_key)
            .query_async(&mut conn)
            .await?;

        self.local_waiters.remove(pairing_name);

        Ok(())
    }

    pub async fn active_count(&self) -> usize {
        // Count keys matching pattern (expensive, use sparingly)
        let mut conn = self.client.clone();
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg("tcpunch:pairing:*")
            .query_async(&mut conn)
            .await
            .unwrap_or_default();

        keys.len()
    }

    pub async fn cleanup_expired(&self) -> usize {
        // Redis TTL handles expiration automatically
        // This just cleans up local waiters for entries that may have expired
        let mut cleaned = 0;
        let mut conn = self.client.clone();

        let to_check: Vec<String> = self.local_waiters.iter().map(|r| r.key().clone()).collect();

        for pairing_name in to_check {
            let pairing_key = Self::pairing_key(&pairing_name);
            let exists: bool = redis::cmd("EXISTS")
                .arg(&pairing_key)
                .query_async(&mut conn)
                .await
                .unwrap_or(false);

            if !exists {
                self.local_waiters.remove(&pairing_name);
                cleaned += 1;
            }
        }

        cleaned
    }
}

/// Enum to hold either registry type
pub enum Registry {
    InMemory(Arc<InMemoryRegistry>),
    Redis(Arc<RedisRegistry>),
}

impl Clone for Registry {
    fn clone(&self) -> Self {
        match self {
            Registry::InMemory(r) => Registry::InMemory(Arc::clone(r)),
            Registry::Redis(r) => Registry::Redis(Arc::clone(r)),
        }
    }
}

impl Registry {
    pub async fn register(
        &self,
        pairing_name: &str,
        client_info: PeerConnectionData,
        reconnect_token: Option<Uuid>,
    ) -> Result<RegistrationResult, RegistryError> {
        match self {
            Registry::InMemory(r) => r.register(pairing_name, client_info, reconnect_token).await,
            Registry::Redis(r) => r.register(pairing_name, client_info, reconnect_token).await,
        }
    }

    pub async fn unregister(&self, pairing_name: &str, token: Uuid) -> Result<(), RegistryError> {
        match self {
            Registry::InMemory(r) => r.unregister(pairing_name, token).await,
            Registry::Redis(r) => r.unregister(pairing_name, token).await,
        }
    }

    pub async fn active_count(&self) -> usize {
        match self {
            Registry::InMemory(r) => r.active_count(),
            Registry::Redis(r) => r.active_count().await,
        }
    }
}
