# TCPunch Rust Server — Deployment Guide

## Build

From the `server/rust/` directory:

```bash
docker build -t tcpunchd .
```

## Deployment Modes

### Single Node (no Redis)

The server uses an in-memory registry. All client pairing state lives on the single instance.

```bash
docker run -p 10000:10000 -p 10001:10001 tcpunchd
```

- No Redis required
- The health check on port 10001 (`/livez`) allows the container orchestrator (ECS, Kubernetes, Fly.io, etc.) to detect an unresponsive container and replace it automatically
- In-flight pairing requests are lost on restart, but these are short-lived — clients will simply reconnect and retry
- Suitable for low-traffic deployments or where brief restart windows are acceptable

### Multi-Node behind a Network Load Balancer (Redis required)

Multiple instances share pairing state via Redis. Without Redis, each node has its own isolated in-memory registry — a client on node 1 would never be matched with a peer on node 2.

```bash
docker run -p 10000:10000 -p 10001:10001 \
  -e REDIS_URL=redis://redis:6379 \
  tcpunchd
```

- Redis is **required** for correct behaviour across nodes
- The NLB can use the `/livez` health check to drain unhealthy targets
- No sticky sessions needed — connections are short-lived (clients connect, get paired, then disconnect), so any node can handle any client
- Suitable for high availability or horizontal scaling

## Ports

| Port | Purpose |
|------|---------|
| `10000` | TCP — client connections |
| `10001` | HTTP — health check (`/livez`) |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TCPUNCH_PORT` | `10000` | TCP listen port |
| `TCPUNCH_HEALTH_PORT` | `10001` | Health check port |
| `REDIS_URL` | *(none)* | Redis connection URL — omit for single-node mode |
| `TCPUNCH_RECV_TIMEOUT` | `5` | Seconds to wait for client request |
| `TCPUNCH_PEER_TIMEOUT` | `300` | Seconds to wait for peer match |
| `TCPUNCH_MAX_CONNECTIONS` | `10000` | Max concurrent connections |
| `RUST_LOG` | `info` | Log level (`trace`, `debug`, `info`, `warn`, `error`) |

## AWS Deployment (Recommended)

Manually spinning up instances and registering them in Route 53 each time is error-prone. The recommended approach on AWS is **ECS Fargate + Network Load Balancer**:

- The NLB has a stable DNS name — register it in Route 53 once and never touch it again
- ECS manages the container lifecycle; if the `/livez` health check fails, ECS automatically replaces the task
- Deploying a new image is a single `ecs update-service` call or a CI/CD push — no manual steps

### Other options

| Option | Trade-off |
|--------|-----------|
| **ECS Fargate + NLB** | Recommended — fully managed, stable DNS, automatic task replacement |
| **EC2 Auto Scaling Group + NLB** | More control over the host, but more operational overhead |
| **Fly.io** | Simplest overall — `fly deploy` handles DNS, health checks, and restarts with minimal config |

### NLB health check configuration

Configure the NLB target group to use the HTTP health check endpoint:

- **Protocol**: HTTP
- **Port**: `10001`
- **Path**: `/livez`
- **Healthy threshold**: 2
- **Unhealthy threshold**: 2
- **Interval**: 30s

### Multi-node on ECS

If running multiple ECS tasks behind the NLB, set `REDIS_URL` via an ECS task definition environment variable or AWS Secrets Manager. See [Multi-Node behind a Network Load Balancer](#multi-node-behind-a-network-load-balancer-redis-required) above.

## Graceful Shutdown

The server handles `SIGTERM` and `SIGINT`. On shutdown it stops accepting new connections and waits up to 30 seconds for active connections to drain before exiting.