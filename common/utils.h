#ifndef HOLEPUNCHINGSERVERCLIENT_UTILS_H
#define HOLEPUNCHINGSERVERCLIENT_UTILS_H

#include <arpa/inet.h>
#include <exception>
#include <string>
#include <cstring>
#include <stdexcept>

// Protocol constants
constexpr size_t MAX_PAIRING_NAME = 100;
constexpr size_t TOKEN_LENGTH = 37;  // UUID string length + null
constexpr size_t CLIENT_REQUEST_SIZE = MAX_PAIRING_NAME + TOKEN_LENGTH + 4;
constexpr size_t SERVER_RESPONSE_SIZE = 1 + 4 + 2 + 4 + 2 + TOKEN_LENGTH;

// Pairing status codes from server
enum class PairingStatus : uint8_t {
    WAITING = 0,   // Waiting for peer to connect
    PAIRED = 1,    // Peer found, connection info provided
    TIMEOUT = 2,   // Server-side timeout, client should reconnect
    ERROR = 3      // Error (invalid token, server overloaded, etc.)
};

// Connection info for a peer (wire format compatible)
typedef struct {
    struct in_addr ip;
    in_port_t      port;
} PeerConnectionData;

// Client request to server (new protocol)
typedef struct {
    char pairing_name[MAX_PAIRING_NAME];
    char reconnect_token[TOKEN_LENGTH];
    uint32_t flags;  // Reserved for future use
} ClientRequest;

// Server response to client (new protocol)
typedef struct {
    uint8_t status;           // PairingStatus
    uint32_t your_ip;         // Network byte order
    uint16_t your_port;       // Network byte order
    uint32_t peer_ip;         // Network byte order (0 if not paired)
    uint16_t peer_port;       // Network byte order (0 if not paired)
    char reconnect_token[TOKEN_LENGTH];
} __attribute__((packed)) ServerResponse;

// Custom exception for timeouts
class Timeout : public std::exception {
public:
    const char* what() const noexcept override {
        return "Operation timed out";
    }
};

inline void error_exit(const std::string& error_string) {
    throw std::runtime_error{error_string};
}

inline void error_exit_errno(const std::string& error_string) {
    if (errno == EAGAIN || errno == EWOULDBLOCK) {
        throw Timeout();
    } else {
        std::string err = error_string + strerror(errno);
        throw std::runtime_error{err};
    }
}

inline std::string ip_to_string(in_addr_t *ip) {
    char str_buffer[INET_ADDRSTRLEN];
    inet_ntop(AF_INET, ip, str_buffer, sizeof(str_buffer));
    return {str_buffer};
}

inline std::string ip_to_string(uint32_t ip) {
    char str_buffer[INET_ADDRSTRLEN];
    inet_ntop(AF_INET, &ip, str_buffer, sizeof(str_buffer));
    return {str_buffer};
}

// Parse server response from wire format
inline bool parse_server_response(const uint8_t* data, size_t len,
                                   PairingStatus& status,
                                   PeerConnectionData& your_info,
                                   PeerConnectionData& peer_info,
                                   std::string& token) {
    if (len < SERVER_RESPONSE_SIZE) {
        return false;
    }

    size_t offset = 0;

    // Status (1 byte)
    status = static_cast<PairingStatus>(data[offset]);
    offset += 1;

    // Your IP (4 bytes, network byte order)
    memcpy(&your_info.ip.s_addr, &data[offset], 4);
    offset += 4;

    // Your port (2 bytes, network byte order)
    memcpy(&your_info.port, &data[offset], 2);
    offset += 2;

    // Peer IP (4 bytes, network byte order)
    memcpy(&peer_info.ip.s_addr, &data[offset], 4);
    offset += 4;

    // Peer port (2 bytes, network byte order)
    memcpy(&peer_info.port, &data[offset], 2);
    offset += 2;

    // Reconnect token (37 bytes, null-terminated)
    token = std::string(reinterpret_cast<const char*>(&data[offset]));

    return true;
}

// Build client request for wire format
inline void build_client_request(uint8_t* buf, const std::string& pairing_name,
                                  const std::string& reconnect_token = "") {
    memset(buf, 0, CLIENT_REQUEST_SIZE);

    // Write pairing name
    size_t name_len = std::min(pairing_name.length(), MAX_PAIRING_NAME - 1);
    memcpy(buf, pairing_name.c_str(), name_len);

    // Write reconnect token
    if (!reconnect_token.empty()) {
        size_t token_len = std::min(reconnect_token.length(), TOKEN_LENGTH - 1);
        memcpy(buf + MAX_PAIRING_NAME, reconnect_token.c_str(), token_len);
    }
}

#endif //HOLEPUNCHINGSERVERCLIENT_UTILS_H
