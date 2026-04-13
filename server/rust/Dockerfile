# Build stage
FROM rust:1.86-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml ./

# Create dummy source to cache dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    echo "pub fn dummy() {}" > src/lib.rs

# Build dependencies (this layer is cached)
RUN cargo build --release && \
    rm -rf src target/release/deps/tcpunchd*

# Copy actual source
COPY src ./src
COPY tests ./tests

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies + AWS CLI
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    unzip \
    jq \
    && curl -s "https://awscli.amazonaws.com/awscli-exe-linux-$(uname -m).zip" -o /tmp/awscliv2.zip \
    && unzip -q /tmp/awscliv2.zip -d /tmp \
    && /tmp/aws/install \
    && rm -rf /tmp/aws /tmp/awscliv2.zip \
    && apt-get purge -y unzip \
    && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -r -s /bin/false tcpunch

WORKDIR /app

# Copy binary and entrypoint from builder
COPY --from=builder /app/target/release/tcpunchd /app/tcpunchd
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

# Set ownership
RUN chown -R tcpunch:tcpunch /app

USER tcpunch

# Expose ports
# 10000 - TCP server for client connections
# 10001 - HTTP health check endpoint
EXPOSE 10000 10001

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:10001/livez || exit 1

# Default environment variables
ENV TCPUNCH_PORT=10000 \
    TCPUNCH_HEALTH_PORT=10001 \
    TCPUNCH_RECV_TIMEOUT=5 \
    TCPUNCH_PEER_TIMEOUT=300 \
    TCPUNCH_MAX_CONNECTIONS=10000 \
    RUST_LOG=info

# Run the server
ENTRYPOINT ["/app/entrypoint.sh"]
