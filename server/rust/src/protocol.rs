//! Wire protocol definitions for TCPunch
//!
//! Binary protocol format (little-endian):
//!
//! ClientRequest (141 bytes):
//!   - pairing_name: [u8; 100] (null-terminated string)
//!   - reconnect_token: [u8; 37] (UUID string or empty, null-terminated)
//!   - flags: u32 (reserved for future use)
//!
//! ServerResponse (51 bytes):
//!   - status: u8 (0=waiting, 1=paired, 2=timeout, 3=error)
//!   - your_ip: u32 (network byte order)
//!   - your_port: u16 (network byte order)
//!   - peer_ip: u32 (network byte order, 0 if not paired)
//!   - peer_port: u16 (network byte order, 0 if not paired)
//!   - reconnect_token: [u8; 37] (UUID string, null-terminated)

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Maximum length of pairing name (including null terminator)
pub const MAX_PAIRING_NAME: usize = 100;

/// Length of reconnect token field (UUID string + null)
pub const TOKEN_LENGTH: usize = 37;

/// Size of client request in bytes
pub const CLIENT_REQUEST_SIZE: usize = MAX_PAIRING_NAME + TOKEN_LENGTH + 4;

/// Size of server response in bytes
pub const SERVER_RESPONSE_SIZE: usize = 1 + 4 + 2 + 4 + 2 + TOKEN_LENGTH;

/// Status codes sent to clients
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PairingStatus {
    /// Waiting for peer to connect
    Waiting = 0,
    /// Peer found, connection info provided
    Paired = 1,
    /// Server-side timeout, client should reconnect
    Timeout = 2,
    /// Error (invalid token, server overloaded, etc.)
    Error = 3,
}

impl From<u8> for PairingStatus {
    fn from(v: u8) -> Self {
        match v {
            0 => PairingStatus::Waiting,
            1 => PairingStatus::Paired,
            2 => PairingStatus::Timeout,
            _ => PairingStatus::Error,
        }
    }
}

/// Connection info for a peer (matches C++ PeerConnectionData layout)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerConnectionData {
    pub ip: Ipv4Addr,
    pub port: u16,
}

impl PeerConnectionData {
    pub fn new(ip: Ipv4Addr, port: u16) -> Self {
        Self { ip, port }
    }

    pub fn from_socket_addr(addr: SocketAddr) -> Option<Self> {
        match addr.ip() {
            IpAddr::V4(ip) => Some(Self { ip, port: addr.port() }),
            IpAddr::V6(_) => None, // IPv6 not supported in current protocol
        }
    }

    pub fn to_socket_addr(self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(self.ip), self.port)
    }
}

/// Request from client to server
#[derive(Debug, Clone)]
pub struct ClientRequest {
    pub pairing_name: String,
    pub reconnect_token: Option<Uuid>,
}

impl ClientRequest {
    /// Parse client request from wire format
    pub fn from_bytes(data: &[u8]) -> Result<Self, ProtocolError> {
        if data.len() < CLIENT_REQUEST_SIZE {
            return Err(ProtocolError::InvalidLength {
                expected: CLIENT_REQUEST_SIZE,
                got: data.len(),
            });
        }

        // Parse pairing name (null-terminated)
        let pairing_name = parse_null_terminated_string(&data[..MAX_PAIRING_NAME])?;
        if pairing_name.is_empty() {
            return Err(ProtocolError::EmptyPairingName);
        }

        // Parse reconnect token (null-terminated, may be empty)
        let token_str = parse_null_terminated_string(&data[MAX_PAIRING_NAME..MAX_PAIRING_NAME + TOKEN_LENGTH])?;
        let reconnect_token = if token_str.is_empty() {
            None
        } else {
            Some(Uuid::parse_str(&token_str).map_err(|_| ProtocolError::InvalidToken)?)
        };

        Ok(Self {
            pairing_name,
            reconnect_token,
        })
    }

    /// Serialize to wire format
    pub fn to_bytes(&self) -> [u8; CLIENT_REQUEST_SIZE] {
        let mut buf = [0u8; CLIENT_REQUEST_SIZE];

        // Write pairing name
        let name_bytes = self.pairing_name.as_bytes();
        let name_len = name_bytes.len().min(MAX_PAIRING_NAME - 1);
        buf[..name_len].copy_from_slice(&name_bytes[..name_len]);

        // Write reconnect token
        if let Some(token) = &self.reconnect_token {
            let token_str = token.to_string();
            let token_bytes = token_str.as_bytes();
            buf[MAX_PAIRING_NAME..MAX_PAIRING_NAME + token_bytes.len()]
                .copy_from_slice(token_bytes);
        }

        buf
    }
}

/// Response from server to client
#[derive(Debug, Clone)]
pub struct ServerResponse {
    pub status: PairingStatus,
    pub your_info: PeerConnectionData,
    pub peer_info: Option<PeerConnectionData>,
    pub reconnect_token: Uuid,
}

impl ServerResponse {
    /// Create a "waiting" response
    pub fn waiting(your_info: PeerConnectionData, token: Uuid) -> Self {
        Self {
            status: PairingStatus::Waiting,
            your_info,
            peer_info: None,
            reconnect_token: token,
        }
    }

    /// Create a "paired" response
    pub fn paired(your_info: PeerConnectionData, peer_info: PeerConnectionData, token: Uuid) -> Self {
        Self {
            status: PairingStatus::Paired,
            your_info,
            peer_info: Some(peer_info),
            reconnect_token: token,
        }
    }

    /// Create a "timeout" response
    pub fn timeout(your_info: PeerConnectionData, token: Uuid) -> Self {
        Self {
            status: PairingStatus::Timeout,
            your_info,
            peer_info: None,
            reconnect_token: token,
        }
    }

    /// Create an "error" response
    pub fn error() -> Self {
        Self {
            status: PairingStatus::Error,
            your_info: PeerConnectionData::new(Ipv4Addr::UNSPECIFIED, 0),
            peer_info: None,
            reconnect_token: Uuid::nil(),
        }
    }

    /// Serialize to wire format
    pub fn to_bytes(&self) -> [u8; SERVER_RESPONSE_SIZE] {
        let mut buf = [0u8; SERVER_RESPONSE_SIZE];
        let mut offset = 0;

        // Status (1 byte)
        buf[offset] = self.status as u8;
        offset += 1;

        // Your IP (4 bytes, network byte order)
        buf[offset..offset + 4].copy_from_slice(&self.your_info.ip.octets());
        offset += 4;

        // Your port (2 bytes, network byte order)
        buf[offset..offset + 2].copy_from_slice(&self.your_info.port.to_be_bytes());
        offset += 2;

        // Peer IP (4 bytes, network byte order)
        let peer_ip = self.peer_info.map(|p| p.ip).unwrap_or(Ipv4Addr::UNSPECIFIED);
        buf[offset..offset + 4].copy_from_slice(&peer_ip.octets());
        offset += 4;

        // Peer port (2 bytes, network byte order)
        let peer_port = self.peer_info.map(|p| p.port).unwrap_or(0);
        buf[offset..offset + 2].copy_from_slice(&peer_port.to_be_bytes());
        offset += 2;

        // Reconnect token (37 bytes, null-terminated string)
        let token_str = self.reconnect_token.to_string();
        buf[offset..offset + token_str.len()].copy_from_slice(token_str.as_bytes());

        buf
    }

    /// Parse from wire format
    pub fn from_bytes(data: &[u8]) -> Result<Self, ProtocolError> {
        if data.len() < SERVER_RESPONSE_SIZE {
            return Err(ProtocolError::InvalidLength {
                expected: SERVER_RESPONSE_SIZE,
                got: data.len(),
            });
        }

        let mut offset = 0;

        // Status
        let status = PairingStatus::from(data[offset]);
        offset += 1;

        // Your IP
        let your_ip = Ipv4Addr::new(data[offset], data[offset + 1], data[offset + 2], data[offset + 3]);
        offset += 4;

        // Your port
        let your_port = u16::from_be_bytes([data[offset], data[offset + 1]]);
        offset += 2;

        // Peer IP
        let peer_ip = Ipv4Addr::new(data[offset], data[offset + 1], data[offset + 2], data[offset + 3]);
        offset += 4;

        // Peer port
        let peer_port = u16::from_be_bytes([data[offset], data[offset + 1]]);
        offset += 2;

        // Reconnect token
        let token_str = parse_null_terminated_string(&data[offset..offset + TOKEN_LENGTH])?;
        let reconnect_token = if token_str.is_empty() {
            Uuid::nil()
        } else {
            Uuid::parse_str(&token_str).map_err(|_| ProtocolError::InvalidToken)?
        };

        let your_info = PeerConnectionData::new(your_ip, your_port);
        let peer_info = if peer_ip.is_unspecified() && peer_port == 0 {
            None
        } else {
            Some(PeerConnectionData::new(peer_ip, peer_port))
        };

        Ok(Self {
            status,
            your_info,
            peer_info,
            reconnect_token,
        })
    }
}

/// Protocol errors
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("Invalid message length: expected {expected}, got {got}")]
    InvalidLength { expected: usize, got: usize },

    #[error("Empty pairing name")]
    EmptyPairingName,

    #[error("Invalid reconnect token")]
    InvalidToken,

    #[error("Invalid UTF-8 in string field")]
    InvalidUtf8,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse a null-terminated string from a byte slice
fn parse_null_terminated_string(data: &[u8]) -> Result<String, ProtocolError> {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8(data[..end].to_vec()).map_err(|_| ProtocolError::InvalidUtf8)
}
