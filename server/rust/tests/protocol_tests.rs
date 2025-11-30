use std::net::Ipv4Addr;
use uuid::Uuid;

use tcpunchd::protocol::{
    ClientRequest, PairingStatus, PeerConnectionData, ServerResponse,
    CLIENT_REQUEST_SIZE, SERVER_RESPONSE_SIZE,
};

#[test]
fn test_client_request_roundtrip() {
    let req = ClientRequest {
        pairing_name: "test_session_123".to_string(),
        reconnect_token: Some(Uuid::new_v4()),
    };

    let bytes = req.to_bytes();
    assert_eq!(bytes.len(), CLIENT_REQUEST_SIZE);

    let parsed = ClientRequest::from_bytes(&bytes).unwrap();
    assert_eq!(req.pairing_name, parsed.pairing_name);
    assert_eq!(req.reconnect_token, parsed.reconnect_token);
}

#[test]
fn test_client_request_without_token() {
    let req = ClientRequest {
        pairing_name: "my_pairing".to_string(),
        reconnect_token: None,
    };

    let bytes = req.to_bytes();
    let parsed = ClientRequest::from_bytes(&bytes).unwrap();

    assert_eq!(req.pairing_name, parsed.pairing_name);
    assert!(parsed.reconnect_token.is_none());
}

#[test]
fn test_client_request_empty_name_fails() {
    let req = ClientRequest {
        pairing_name: "".to_string(),
        reconnect_token: None,
    };

    let bytes = req.to_bytes();
    assert!(ClientRequest::from_bytes(&bytes).is_err());
}

#[test]
fn test_server_response_waiting_roundtrip() {
    let token = Uuid::new_v4();
    let resp = ServerResponse::waiting(
        PeerConnectionData::new(Ipv4Addr::new(192, 168, 1, 100), 12345),
        token,
    );

    let bytes = resp.to_bytes();
    assert_eq!(bytes.len(), SERVER_RESPONSE_SIZE);

    let parsed = ServerResponse::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.status, PairingStatus::Waiting);
    assert_eq!(parsed.your_info.ip, Ipv4Addr::new(192, 168, 1, 100));
    assert_eq!(parsed.your_info.port, 12345);
    assert!(parsed.peer_info.is_none());
    assert_eq!(parsed.reconnect_token, token);
}

#[test]
fn test_server_response_paired_roundtrip() {
    let token = Uuid::new_v4();
    let resp = ServerResponse::paired(
        PeerConnectionData::new(Ipv4Addr::new(192, 168, 1, 100), 12345),
        PeerConnectionData::new(Ipv4Addr::new(10, 0, 0, 50), 54321),
        token,
    );

    let bytes = resp.to_bytes();
    let parsed = ServerResponse::from_bytes(&bytes).unwrap();

    assert_eq!(parsed.status, PairingStatus::Paired);
    assert_eq!(parsed.your_info.ip, Ipv4Addr::new(192, 168, 1, 100));
    assert_eq!(parsed.your_info.port, 12345);

    let peer = parsed.peer_info.unwrap();
    assert_eq!(peer.ip, Ipv4Addr::new(10, 0, 0, 50));
    assert_eq!(peer.port, 54321);
    assert_eq!(parsed.reconnect_token, token);
}

#[test]
fn test_server_response_error() {
    let resp = ServerResponse::error();
    let bytes = resp.to_bytes();
    let parsed = ServerResponse::from_bytes(&bytes).unwrap();

    assert_eq!(parsed.status, PairingStatus::Error);
    assert!(parsed.peer_info.is_none());
}

#[test]
fn test_peer_connection_data_socket_addr_conversion() {
    use std::net::SocketAddr;

    let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
    let peer_data = PeerConnectionData::from_socket_addr(addr).unwrap();

    assert_eq!(peer_data.ip, Ipv4Addr::new(192, 168, 1, 1));
    assert_eq!(peer_data.port, 8080);
    assert_eq!(peer_data.to_socket_addr(), addr);
}

#[test]
fn test_pairing_status_from_u8() {
    assert_eq!(PairingStatus::from(0), PairingStatus::Waiting);
    assert_eq!(PairingStatus::from(1), PairingStatus::Paired);
    assert_eq!(PairingStatus::from(2), PairingStatus::Timeout);
    assert_eq!(PairingStatus::from(3), PairingStatus::Error);
    assert_eq!(PairingStatus::from(255), PairingStatus::Error);
}
