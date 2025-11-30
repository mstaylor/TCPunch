# Client Migration Guide: TCPunch Protocol v2

This document describes the changes clients need to make to work with the new Rust-based TCPunch server.

## Protocol Changes Summary

| Aspect | Old Protocol | New Protocol |
|--------|--------------|--------------|
| Request size | Variable (string) | Fixed 141 bytes |
| Response size | 6 bytes | 51 bytes |
| Reconnect support | No | Yes (UUID token) |
| Status indication | Implicit | Explicit status byte |
| Timeout handling | Client blocks forever | Server sends timeout status |

---

## Wire Format

### Client Request (141 bytes)

```
Offset  Size  Field             Description
------  ----  -----             -----------
0       100   pairing_name      Null-terminated UTF-8 string
100     37    reconnect_token   UUID string (e.g., "550e8400-e29b-41d4-a716-446655440000") or empty
137     4     flags             Reserved (set to 0)
```

### Server Response (51 bytes)

```
Offset  Size  Field             Description
------  ----  -----             -----------
0       1     status            0=waiting, 1=paired, 2=timeout, 3=error
1       4     your_ip           Client's public IP (network byte order)
5       2     your_port         Client's public port (network byte order)
7       4     peer_ip           Peer's IP (network byte order, 0 if not paired)
11      2     peer_port         Peer's port (network byte order, 0 if not paired)
13      37    reconnect_token   UUID string for reconnection
```

---

## Status Codes

| Value | Name | Meaning | Client Action |
|-------|------|---------|---------------|
| 0 | WAITING | Registered, waiting for peer | Wait for second response |
| 1 | PAIRED | Peer found | Proceed to hole punching |
| 2 | TIMEOUT | Server-side timeout | Reconnect with token |
| 3 | ERROR | Invalid request/token | Start fresh (clear token) |

---

## Connection Flow

### New Flow (with reconnection support)

```
CLIENT                                 SERVER
   │                                      │
   │──── ClientRequest ──────────────────▶│
   │     (pairing_name, token=empty)      │
   │                                      │
   │◀──── ServerResponse ────────────────│
   │      status=WAITING                  │
   │      your_ip, your_port              │
   │      reconnect_token="abc-123..."    │
   │                                      │
   │      ... waiting for peer ...        │
   │                                      │
   │◀──── ServerResponse ────────────────│
   │      status=PAIRED                   │
   │      peer_ip, peer_port              │
   │                                      │
   └──── hole punch to peer ─────────────┘
```

### Reconnection Flow (after disconnect/timeout)

```
CLIENT                                 SERVER
   │                                      │
   │──── ClientRequest ──────────────────▶│
   │     (pairing_name, token="abc-123")  │
   │                                      │
   │◀──── ServerResponse ────────────────│
   │      status=WAITING (resumed)        │
   │      reconnect_token="abc-123..."    │
   │                                      │
   │      ... continue waiting ...        │
```

---

## Code Changes Required

### Rust Client

```rust
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

const MAX_PAIRING_NAME: usize = 100;
const TOKEN_LENGTH: usize = 37;
const CLIENT_REQUEST_SIZE: usize = 141;
const SERVER_RESPONSE_SIZE: usize = 51;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum PairingStatus {
    Waiting = 0,
    Paired = 1,
    Timeout = 2,
    Error = 3,
}

impl From<u8> for PairingStatus {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::Waiting,
            1 => Self::Paired,
            2 => Self::Timeout,
            _ => Self::Error,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub ip: Ipv4Addr,
    pub port: u16,
}

#[derive(Debug)]
pub struct ServerResponse {
    pub status: PairingStatus,
    pub your_info: PeerInfo,
    pub peer_info: Option<PeerInfo>,
    pub token: String,
}

fn build_request(pairing_name: &str, token: Option<&str>) -> [u8; CLIENT_REQUEST_SIZE] {
    let mut buf = [0u8; CLIENT_REQUEST_SIZE];

    // Write pairing name
    let name_bytes = pairing_name.as_bytes();
    let len = name_bytes.len().min(MAX_PAIRING_NAME - 1);
    buf[..len].copy_from_slice(&name_bytes[..len]);

    // Write token if present
    if let Some(t) = token {
        let token_bytes = t.as_bytes();
        let len = token_bytes.len().min(TOKEN_LENGTH - 1);
        buf[MAX_PAIRING_NAME..MAX_PAIRING_NAME + len].copy_from_slice(&token_bytes[..len]);
    }

    buf
}

fn parse_response(buf: &[u8; SERVER_RESPONSE_SIZE]) -> ServerResponse {
    let status = PairingStatus::from(buf[0]);

    let your_ip = Ipv4Addr::new(buf[1], buf[2], buf[3], buf[4]);
    let your_port = u16::from_be_bytes([buf[5], buf[6]]);

    let peer_ip = Ipv4Addr::new(buf[7], buf[8], buf[9], buf[10]);
    let peer_port = u16::from_be_bytes([buf[11], buf[12]]);

    let token_end = buf[13..50].iter().position(|&b| b == 0).unwrap_or(37);
    let token = String::from_utf8_lossy(&buf[13..13 + token_end]).to_string();

    let peer_info = if peer_ip.is_unspecified() && peer_port == 0 {
        None
    } else {
        Some(PeerInfo { ip: peer_ip, port: peer_port })
    };

    ServerResponse {
        status,
        your_info: PeerInfo { ip: your_ip, port: your_port },
        peer_info,
        token,
    }
}

pub fn pair(
    pairing_name: &str,
    server_addr: &str,
    timeout: Duration,
    max_retries: u32,
) -> Result<(PeerInfo, PeerInfo), String> {
    let mut reconnect_token: Option<String> = None;

    for attempt in 0..max_retries {
        // Connect to server
        let mut stream = TcpStream::connect(server_addr)
            .map_err(|e| format!("Connect failed: {}", e))?;

        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(timeout)).ok();

        // Send request
        let request = build_request(pairing_name, reconnect_token.as_deref());
        stream.write_all(&request)
            .map_err(|e| format!("Send failed: {}", e))?;

        // Receive response
        let mut resp_buf = [0u8; SERVER_RESPONSE_SIZE];
        stream.read_exact(&mut resp_buf)
            .map_err(|e| format!("Receive failed: {}", e))?;

        let resp = parse_response(&resp_buf);
        reconnect_token = Some(resp.token.clone());

        match resp.status {
            PairingStatus::Paired => {
                let peer = resp.peer_info.ok_or("No peer info in PAIRED response")?;
                return Ok((resp.your_info, peer));
            }

            PairingStatus::Waiting => {
                // Wait for second response with peer info
                if let Err(e) = stream.read_exact(&mut resp_buf) {
                    eprintln!("Timeout waiting for peer (attempt {}): {}", attempt + 1, e);
                    continue;
                }

                let resp = parse_response(&resp_buf);
                if resp.status == PairingStatus::Paired {
                    let peer = resp.peer_info.ok_or("No peer info")?;
                    return Ok((resp.your_info, peer));
                }
                // Timeout or error, retry
            }

            PairingStatus::Timeout => {
                eprintln!("Server timeout (attempt {})", attempt + 1);
                continue;
            }

            PairingStatus::Error => {
                eprintln!("Server error, clearing token (attempt {})", attempt + 1);
                reconnect_token = None;
                continue;
            }
        }
    }

    Err(format!("Failed to pair after {} retries", max_retries))
}

// Example usage
fn main() {
    match pair("my-session-123", "127.0.0.1:10000", Duration::from_secs(60), 3) {
        Ok((your_info, peer_info)) => {
            println!("Paired successfully!");
            println!("  Your address: {}:{}", your_info.ip, your_info.port);
            println!("  Peer address: {}:{}", peer_info.ip, peer_info.port);
            // Now perform hole punching...
        }
        Err(e) => {
            eprintln!("Pairing failed: {}", e);
        }
    }
}
```

---

### C/C++ Client

#### 1. Define Protocol Constants

```cpp
constexpr size_t MAX_PAIRING_NAME = 100;
constexpr size_t TOKEN_LENGTH = 37;
constexpr size_t CLIENT_REQUEST_SIZE = 141;  // 100 + 37 + 4
constexpr size_t SERVER_RESPONSE_SIZE = 51;  // 1 + 4 + 2 + 4 + 2 + 37

enum class PairingStatus : uint8_t {
    WAITING = 0,
    PAIRED = 1,
    TIMEOUT = 2,
    ERROR = 3
};
```

#### 2. Build Client Request

```cpp
void build_request(uint8_t* buf, const std::string& pairing_name,
                   const std::string& token = "") {
    memset(buf, 0, CLIENT_REQUEST_SIZE);

    // Copy pairing name (max 99 chars + null)
    size_t len = std::min(pairing_name.length(), (size_t)99);
    memcpy(buf, pairing_name.c_str(), len);

    // Copy reconnect token if present
    if (!token.empty()) {
        memcpy(buf + MAX_PAIRING_NAME, token.c_str(),
               std::min(token.length(), (size_t)36));
    }
}
```

#### 3. Parse Server Response

```cpp
struct ServerResponse {
    PairingStatus status;
    uint32_t your_ip;    // Network byte order
    uint16_t your_port;  // Network byte order
    uint32_t peer_ip;    // Network byte order
    uint16_t peer_port;  // Network byte order
    char token[37];
};

bool parse_response(const uint8_t* buf, ServerResponse& resp) {
    resp.status = static_cast<PairingStatus>(buf[0]);
    memcpy(&resp.your_ip, buf + 1, 4);
    memcpy(&resp.your_port, buf + 5, 2);
    memcpy(&resp.peer_ip, buf + 7, 4);
    memcpy(&resp.peer_port, buf + 11, 2);
    memcpy(resp.token, buf + 13, 37);
    resp.token[36] = '\0';
    return true;
}
```

#### 4. Updated pair() Function (Pseudocode)

```cpp
int pair(const std::string& pairing_name, const std::string& server,
         int port, int timeout_ms, int max_retries = 3) {

    std::string reconnect_token;

    for (int attempt = 0; attempt < max_retries; attempt++) {
        int sock = connect_to_server(server, port, timeout_ms);

        // Send request
        uint8_t req[CLIENT_REQUEST_SIZE];
        build_request(req, pairing_name, reconnect_token);
        send(sock, req, CLIENT_REQUEST_SIZE, 0);

        // Receive response
        uint8_t resp_buf[SERVER_RESPONSE_SIZE];
        recv(sock, resp_buf, SERVER_RESPONSE_SIZE, MSG_WAITALL);

        ServerResponse resp;
        parse_response(resp_buf, resp);

        // Save token for reconnection
        reconnect_token = resp.token;

        switch (resp.status) {
            case PairingStatus::PAIRED:
                // Got peer immediately
                return do_hole_punch(resp.your_ip, resp.your_port,
                                     resp.peer_ip, resp.peer_port, sock);

            case PairingStatus::WAITING:
                // Wait for second response with peer info
                recv(sock, resp_buf, SERVER_RESPONSE_SIZE, MSG_WAITALL);
                parse_response(resp_buf, resp);

                if (resp.status == PairingStatus::PAIRED) {
                    return do_hole_punch(...);
                }
                // Fall through to retry
                break;

            case PairingStatus::TIMEOUT:
                // Server timeout, retry with token
                close(sock);
                continue;

            case PairingStatus::ERROR:
                // Invalid token, start fresh
                reconnect_token.clear();
                close(sock);
                continue;
        }
    }

    throw std::runtime_error("Failed to pair after retries");
}
```

---

### Python Client

```python
import struct
import socket

MAX_PAIRING_NAME = 100
TOKEN_LENGTH = 37
CLIENT_REQUEST_SIZE = 141
SERVER_RESPONSE_SIZE = 51

class PairingStatus:
    WAITING = 0
    PAIRED = 1
    TIMEOUT = 2
    ERROR = 3

def build_request(pairing_name: str, token: str = "") -> bytes:
    buf = bytearray(CLIENT_REQUEST_SIZE)
    name_bytes = pairing_name.encode('utf-8')[:99]
    buf[:len(name_bytes)] = name_bytes
    if token:
        token_bytes = token.encode('utf-8')[:36]
        buf[MAX_PAIRING_NAME:MAX_PAIRING_NAME + len(token_bytes)] = token_bytes
    return bytes(buf)

def parse_response(data: bytes) -> dict:
    status = data[0]
    your_ip = socket.inet_ntoa(data[1:5])
    your_port = struct.unpack('!H', data[5:7])[0]
    peer_ip = socket.inet_ntoa(data[7:11])
    peer_port = struct.unpack('!H', data[11:13])[0]
    token = data[13:50].split(b'\x00')[0].decode('utf-8')

    return {
        'status': status,
        'your_ip': your_ip,
        'your_port': your_port,
        'peer_ip': peer_ip,
        'peer_port': peer_port,
        'token': token
    }
```

---

### Go Client

```go
const (
    MaxPairingName     = 100
    TokenLength        = 37
    ClientRequestSize  = 141
    ServerResponseSize = 51
)

type PairingStatus uint8

const (
    StatusWaiting PairingStatus = 0
    StatusPaired  PairingStatus = 1
    StatusTimeout PairingStatus = 2
    StatusError   PairingStatus = 3
)

type ServerResponse struct {
    Status    PairingStatus
    YourIP    net.IP
    YourPort  uint16
    PeerIP    net.IP
    PeerPort  uint16
    Token     string
}

func BuildRequest(pairingName, token string) []byte {
    buf := make([]byte, ClientRequestSize)
    copy(buf[:MaxPairingName], pairingName)
    if token != "" {
        copy(buf[MaxPairingName:], token)
    }
    return buf
}

func ParseResponse(data []byte) (*ServerResponse, error) {
    if len(data) < ServerResponseSize {
        return nil, errors.New("response too short")
    }

    return &ServerResponse{
        Status:   PairingStatus(data[0]),
        YourIP:   net.IP(data[1:5]),
        YourPort: binary.BigEndian.Uint16(data[5:7]),
        PeerIP:   net.IP(data[7:11]),
        PeerPort: binary.BigEndian.Uint16(data[11:13]),
        Token:    string(bytes.TrimRight(data[13:50], "\x00")),
    }, nil
}
```

---

## Timeout Recommendations

| Operation | Recommended Timeout |
|-----------|-------------------|
| Connect to server | 5 seconds |
| Receive response | 5 seconds |
| Wait for peer | 60 seconds (configurable on server) |
| Hole punch attempts | 30 seconds |
| Retry backoff | 1-5 seconds between attempts |

---

## Backward Compatibility

The new server is **NOT backward compatible** with old clients. Clients must be updated to use the new protocol.

If you need to support both old and new clients during migration:
1. Run both servers on different ports
2. Update clients to try new server first, fall back to old
3. Once all clients are updated, decommission old server

---

## Testing Your Implementation

You can test your client against the server using these checks:

1. **Basic pairing**: Two clients with same pairing name should receive each other's info
2. **Reconnection**: Disconnect client A, reconnect with token, client B should still find it
3. **Timeout**: Single client should receive TIMEOUT status after server timeout
4. **Invalid token**: Using wrong token should receive ERROR status

Server health check endpoint: `http://<server>:10001/health`
