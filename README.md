# ⚡ rushsocket

**Stop paying for Pusher. One Rust binary, full Pusher Protocol v7, every client SDK just works. Deploy in seconds, scale with Redis, own your real-time infrastructure.**

[![Website](https://img.shields.io/badge/website-rushsocket.xyz-00ff41)](https://rushsocket.xyz)
[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/built%20with-Rust-orange)](https://www.rust-lang.org/)
[![Pusher Protocol](https://img.shields.io/badge/Pusher%20Protocol-v7-brightgreen)](https://pusher.com/docs/channels/library_auth_reference/pusher-websockets-protocol/)
[![WebSocket](https://img.shields.io/badge/WebSocket-RFC%206455-blue)](https://datatracker.ietf.org/doc/html/rfc6455)

---

## 📑 Table of Contents

- [Why rushsocket?](#-why-rushsocket)
- [Features](#-features)
- [Installation](#-installation)
- [Configuration](#%EF%B8%8F-configuration)
- [CLI Usage](#-cli-usage)
- [HTTP REST API](#-http-rest-api)
- [Webhooks](#-webhooks)
- [Health & Readiness](#-health--readiness)
- [Stats](#-stats)
- [Horizontal Scaling with Redis](#-horizontal-scaling-with-redis)
- [TLS / SSL](#-tls--ssl)
- [Cloudflare (WSS Proxy)](#-cloudflare-wss-proxy)
- [Channel Types](#-channel-types)
- [Architecture](#-architecture)
- [Project Structure](#-project-structure)
- [License](#-license)

---

## 🤔 Why rushsocket?

Built in Rust. No garbage collector, no runtime, no bloat. One binary that handles thousands of concurrent WebSocket connections while barely touching your RAM. Your Pusher SDK already works with it — just change the host and you're done.

| Feature | **rushsocket** | Pusher (cloud) |
|---|---|---|
| Language | Rust 🦀 | Proprietary |
| Single binary | ✅ | N/A |
| Pusher Protocol v7 | ✅ | ✅ |
| Webhooks | ✅ | ✅ |
| Redis scaling | ✅ | N/A |
| Encrypted channels | ✅ | ✅ |
| Cache channels | ✅ | ✅ |
| TLS/SSL built-in | ✅ | ✅ |
| Self-hosted | ✅ | ❌ |
| Cost | 🆓 Free | 💰 Paid |

---

## 🚀 Features

- 🦀 **Written in Rust** — single binary deployment, uses jemalloc on Linux for optimal memory performance
- 📡 **Pusher Protocol v7** — fully compatible with every Pusher client SDK (JavaScript, PHP, Python, Ruby, Go, etc.)
- 🔄 **Drop-in replacement** — swap out Pusher with zero client-side changes
- 📢 **All channel types** — public, private, presence, encrypted-private, and cache channels
- 🌐 **HTTP REST API** — trigger events, batch events, query channels, manage connections, with body MD5 integrity verification
- 🪝 **Webhooks** — Pusher-compatible webhook delivery with HMAC-SHA256 signing, batching, and retries
- 🛡️ **Rate limiting** — per-app limits on connections, events/sec, and read requests/sec
- 📈 **Horizontal scaling** — Redis adapter with pub/sub broadcasting, global presence via Redis Hashes, and stats heartbeats
- 🔒 **TLS/SSL** — built-in via rustls, no external proxy required
- 🐧 **Systemd support** — Type=notify integration with sd-notify for reliable service management
- 📊 **Rich stats endpoint** — server, process, system, and per-app metrics
- 💚 **Health & readiness endpoints** — `/health` for liveness, `/ready` for traffic readiness (adapter-aware), `/accept-traffic` for graceful shutdown
- 🛑 **Graceful shutdown** — configurable drain and grace periods to close connections cleanly
- 🧠 **Memory protection** — configurable memory threshold to reject new connections before OOM
- 💾 **Cache channels** — configurable TTL for replaying the last event to new subscribers
- 👤 **User authentication** — `pusher:signin` support with server-side user tracking and terminate API
- 🐢 **Slow consumer protection** — connections that can't keep up are automatically disconnected
- ⚛️ **Atomic admission control** — race-free connection and presence member limits using lock-free CAS operations
- 🔐 **Lock safety** — uses `parking_lot::Mutex` (non-poisoning) for per-connection state

---

## 📦 Installation

### ⚡ Quick Install (Recommended)

One command — downloads the latest pre-built binary, installs it, sets up config, and enables the systemd service:

```bash
curl -sSfL https://raw.githubusercontent.com/rushsocket/rushsocket/main/install.sh | sudo bash
```

To install a specific version:

```bash
curl -sSfL https://raw.githubusercontent.com/rushsocket/rushsocket/main/install.sh | sudo bash -s -- v1.0.0
```

Pre-built binaries are available for **x86_64** and **aarch64** Linux. No Rust toolchain required.

### 🔨 Build from Source

For unsupported architectures or custom builds:

```bash
git clone https://github.com/rushsocket/rushsocket.git
cd rushsocket
cargo build --release
sudo cp target/release/rushsocket /usr/local/bin/
```

### 🐧 Systemd (Linux)

The install script automatically sets up systemd with `Type=notify` for reliable readiness signaling. To manage the service:

```bash
systemctl status rushsocket     # Check status
systemctl restart rushsocket    # Restart
journalctl -u rushsocket -f     # View logs
```

To uninstall (stops service, removes binary and service file):

```bash
curl -sSfL https://raw.githubusercontent.com/rushsocket/rushsocket/main/uninstall.sh | sudo bash
```

To also remove the config directory (`/etc/rushsocket`):

```bash
curl -sSfL https://raw.githubusercontent.com/rushsocket/rushsocket/main/uninstall.sh | sudo bash -s -- --purge
```

The install script will:
- 📥 Download the correct binary for your architecture (or build from source)
- 📁 Install it to `/usr/local/bin/rushsocket`
- 📝 Create a default config at `/etc/rushsocket/config.toml` (preserves existing)
- ⚙️ Register and enable the systemd service

---

## ⚙️ Configuration

Running `rushsocket` without flags uses built-in defaults (app key: `rushsocket-key`, secret: `rushsocket-secret`). Use `--config` to load a custom config file.

### 📋 Full Configuration Reference

```toml
# =============================================================================
# rushsocket configuration
# =============================================================================
#
# Use -1 for unlimited on rate limits and connection caps.
# Load with: rushsocket --config /path/to/config.toml

# --- Server ---
host = "0.0.0.0"                    # IP literal only (not a hostname)
port = 6001
metrics_enabled = true
metrics_port = 9100
mode = "full"
debug = false

# --- HTTP hardening ---
max_http_event_body_kb = 64          # Body limit for POST /events (KB)
max_http_batch_body_kb = 1280        # Body limit for POST /batch_events (KB)
max_concurrent_http_requests = 256   # Max concurrent API requests (0 = unlimited)

# --- Connection lifecycle ---
activity_timeout = 120
server_ping_interval = 60
server_pong_timeout = 15
shutdown_drain_period = 10           # Seconds to drain WS connections after SIGTERM
shutdown_grace_period = 6            # Hard timeout for in-flight HTTP after drain

# --- Memory ---
memory_threshold_percent = 90.0

# --- Cache ---
cache_ttl = 3600

# --- Adapter (horizontal scaling) ---
adapter = "local"    # "local" or "redis"

[redis]
url = "redis://127.0.0.1:6379"
prefix = "rushsocket"
request_timeout = 5000

# --- TLS / SSL ---
[ssl]
enabled = false
cert_path = "/path/to/cert.pem"
key_path = "/path/to/key.pem"

# --- Apps ---
[[apps]]
id = "my-app-id"
key = "my-app-key"
secret = "my-app-secret"
enabled = true
client_event_mode = "all"           # "all", "members", or "none"
enable_user_authentication = true
enable_cache_channels = true
allowed_origins = [                  # empty = allow all
    "https://example.com",
    "https://app.example.com",
    "http://localhost:3000",
]

# Webhooks
webhook_url = "https://example.com/webhooks"
webhook_batch_ms = 1000              # Batching interval (ms)
enable_subscription_count_webhook = false

# Rate limits (per second, -1 = unlimited)
max_connections = -1
max_backend_events_per_sec = -1
max_client_events_per_sec = -1
max_read_req_per_sec = -1

# Size limits
max_presence_members_per_channel = 200
max_presence_member_size_in_kb = 2
max_channel_name_length = 256
max_event_channels_at_once = 200
max_event_name_length = 256
max_event_payload_in_kb = 32
max_event_batch_size = 20
max_message_size_in_kb = 40
```

### 📖 Configuration Option Reference

#### 🖥️ Server Options

| Option | Default | Description |
|---|---|---|
| `host` | `"0.0.0.0"` | Bind address (IP literal only, not a hostname) |
| `port` | `6001` | WebSocket + HTTP API port |
| `metrics_enabled` | `true` | Enable the stats/health server |
| `metrics_port` | `9100` | Stats server port |
| `mode` | `"full"` | Server mode |
| `debug` | `false` | Enable verbose debug logging |
| `max_http_event_body_kb` | `64` | Max POST body size for `/events` (KB) |
| `max_http_batch_body_kb` | `1280` | Max POST body size for `/batch_events` (KB) |
| `max_concurrent_http_requests` | `256` | Max concurrent API requests; 0 = unlimited. Health routes are never limited. |

#### ⏱️ Connection Lifecycle

| Option | Default | Description |
|---|---|---|
| `activity_timeout` | `120` | Seconds before an idle connection is closed |
| `server_ping_interval` | `60` | How often the server sends pings (seconds) |
| `server_pong_timeout` | `15` | Seconds to wait for a pong before closing |
| `shutdown_drain_period` | `10` | Seconds to wait for WS connections to drain after SIGTERM |
| `shutdown_grace_period` | `6` | Hard timeout for in-flight HTTP requests after drain period |

#### 🧠 Memory & Cache

| Option | Default | Description |
|---|---|---|
| `memory_threshold_percent` | `90.0` | Reject new connections above this memory usage %. Sampled every 2s from `/proc` (Linux) or Win32 API (Windows), never blocking the connection handshake. |
| `cache_ttl` | `3600` | Cache channel TTL in seconds |

#### 🔧 Per-App Options

| Option | Default | Description |
|---|---|---|
| `id` | — | Unique app identifier |
| `key` | — | Public app key (used by clients) |
| `secret` | — | Private app secret (used for HMAC signing) |
| `enabled` | `true` | Enable or disable the app |
| `client_event_mode` | `"all"` | Allow client events: `"all"` (any subscribed client), `"members"` (only presence channel members), or `"none"` (disabled) |
| `enable_user_authentication` | `true` | Enable `pusher:signin` |
| `enable_cache_channels` | `true` | Enable cache channel replay for new subscribers |
| `allowed_origins` | `[]` | CORS origins — empty allows all |
| `webhook_url` | — | URL to receive webhook POST requests |
| `webhook_batch_ms` | `1000` | Webhook batching interval in milliseconds |
| `enable_subscription_count_webhook` | `false` | Enable `subscription_count` webhook events |
| `max_connections` | `-1` | Max concurrent connections per node (-1 = unlimited) |
| `max_backend_events_per_sec` | `-1` | HTTP API event rate limit |
| `max_client_events_per_sec` | `-1` | Client-to-server event rate limit |
| `max_read_req_per_sec` | `-1` | HTTP GET request rate limit |
| `max_presence_members_per_channel` | `200` | Maximum unique users in a presence channel |
| `max_presence_member_size_in_kb` | `2` | Max size of a single presence member's channel_data JSON |
| `max_channel_name_length` | `256` | Maximum channel name length |
| `max_event_channels_at_once` | `200` | Max channels per trigger request |
| `max_event_name_length` | `256` | Maximum event name length |
| `max_event_payload_in_kb` | `32` | Max event data payload size |
| `max_event_batch_size` | `20` | Max events per batch trigger request |
| `max_message_size_in_kb` | `40` | Max WebSocket message frame size |

---

## 💻 CLI Usage

```
rushsocket [OPTIONS]

Options:
  -c, --config <PATH>  Path to config file
  -h, --help           Print help
```

**Examples:**

```bash
# Use default config lookup
rushsocket

# Use a specific config file
rushsocket --config /etc/rushsocket/config.toml
```

---

## 🌐 HTTP REST API

rushsocket exposes a Pusher-compatible HTTP REST API on the main port (`6001` by default). All endpoints use HMAC-SHA256 authentication, identical to the Pusher HTTP API.

| Method | Endpoint | Description |
|---|---|---|
| `POST` | `/apps/{app_id}/events` | 📤 Trigger a single event |
| `POST` | `/apps/{app_id}/batch_events` | 📦 Trigger multiple events in one request |
| `GET` | `/apps/{app_id}/channels` | 📋 List all active channels |
| `GET` | `/apps/{app_id}/channels/{channel}` | 🔍 Get channel info (subscriber count, etc.) |
| `GET` | `/apps/{app_id}/channels/{channel}/users` | 👥 List presence channel members |
| `GET` | `/apps/{app_id}/connections` | 🔗 Get total connection count |
| `POST` | `/apps/{app_id}/users/{user_id}/terminate_connections` | ⛔ Disconnect a user by ID |

### 🔑 Authentication

All requests must include query parameters: `auth_key`, `auth_timestamp`, `auth_version`, `auth_signature`, and `body_md5` (for POST requests with a body). The signature is computed as:

```
HMAC-SHA256(secret, "METHOD\n/apps/{app_id}/endpoint\n{sorted_query_params}")
```

POST requests with a `body_md5` query parameter are verified — if the MD5 hash of the request body does not match, the request is rejected with `400 Bad Request`.

### 📤 Trigger an Event

```bash
curl -X POST "http://localhost:6001/apps/my-app-id/events?auth_key=...&auth_signature=..." \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-event",
    "channel": "my-channel",
    "data": "{\"message\": \"Hello, world!\"}"
  }'
```

Any existing Pusher server SDK works out of the box — just point it at your rushsocket host.

---

## 🪝 Webhooks

rushsocket sends Pusher-compatible webhook notifications to your server when events occur. Configure a `webhook_url` per app to receive them.

### 📬 Delivery

- 📦 Events are batched by the `webhook_batch_ms` interval (default: 1000ms) and sent as a single POST request
- 🔏 Each request is signed with HMAC-SHA256 using the app's secret
- 🔁 Failed deliveries are retried up to 3 times with exponential backoff (2s, 4s, 8s)
- ⚡ Webhook delivery is non-blocking — it never slows down the WebSocket hot path

### 📋 Headers

| Header | Description |
|---|---|
| `Content-Type` | `application/json` |
| `X-Pusher-Key` | The app key |
| `X-Pusher-Signature` | HMAC-SHA256 hex digest of the request body, signed with the app secret |

### 📄 Payload Format

```json
{
  "time_ms": 1700000000000,
  "events": [
    { "name": "channel_occupied", "channel": "my-channel" }
  ]
}
```

### 📡 Event Types

#### 🏠 Channel Existence Events

| Event | Trigger | Payload |
|---|---|---|
| `channel_occupied` | First subscriber joins a channel | `{"name": "channel_occupied", "channel": "my-channel"}` |
| `channel_vacated` | Last subscriber leaves a channel | `{"name": "channel_vacated", "channel": "my-channel"}` |

In Redis cluster mode, these are deduplicated globally — `channel_occupied` fires only once across all nodes (via Redis SETNX), and `channel_vacated` fires only when no node has any subscribers (verified with a real-time cross-node query).

#### 💾 Cache Channel Events

| Event | Trigger | Payload |
|---|---|---|
| `cache_miss` | Client subscribes to a cache channel with no cached message | `{"name": "cache_miss", "channel": "cache-my-channel"}` |

#### 👥 Presence Events

| Event | Trigger | Payload |
|---|---|---|
| `member_added` | New user subscribes to a presence channel | `{"name": "member_added", "channel": "presence-room", "user_id": "user-42"}` |
| `member_removed` | User unsubscribes from a presence channel (all sockets disconnected) | `{"name": "member_removed", "channel": "presence-room", "user_id": "user-42"}` |

In Redis cluster mode, presence state is stored globally in Redis Hashes. A user is considered "removed" only when they have no sockets on any node.

#### 💬 Client Events

| Event | Trigger | Payload |
|---|---|---|
| `client_event` | Client sends a `client-*` event on a private or presence channel | `{"name": "client_event", "channel": "private-chat", "event": "client-typing", "data": "...", "socket_id": "1.1", "user_id": "user-42"}` |

The `user_id` field is only present for presence channels. The `socket_id` identifies the sending connection.

#### 🔢 Subscription Count Events

| Event | Trigger | Payload |
|---|---|---|
| `subscription_count` | Subscriber count changes on a non-presence channel | `{"name": "subscription_count", "channel": "my-channel", "subscription_count": 5}` |

Subscription count webhooks are **disabled by default**. Enable per-app with:

```toml
enable_subscription_count_webhook = true
```

Per the Pusher specification, this event is not sent for presence channels.

### ✅ Signature Verification

To verify webhooks on your server:

```python
import hmac, hashlib

def verify_webhook(secret, signature, body):
    expected = hmac.new(secret.encode(), body.encode(), hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, signature)
```

```php
$expected = hash_hmac('sha256', $body, $secret);
$valid = hash_equals($expected, $signature);
```

---

## 💚 Health & Readiness

Health and readiness endpoints are available on the main port (`6001`).

### `GET /`

🟢 Liveness check. Always returns `200 OK`.

### `GET /ready`

🔍 Readiness check for load balancers and orchestration (e.g., Kubernetes readiness probes).

- **Local adapter:** Always returns `200 OK` immediately.
- **Redis adapter:** Returns `503 Not ready` until Redis subscriptions are established and peer discovery is complete. Returns `503` again if the node is shutting down or if the Redis connection drops (readiness drops on disconnect and recovers after resubscribe + re-discovery).

### `GET /accept-traffic`

🚦 Returns `200 OK` unless the server is in graceful shutdown, in which case it returns `503`. Useful for draining traffic before termination.

---

## 📊 Stats

The stats server runs on a separate port (`9100` by default, configurable via `metrics_port`). Only available when `metrics_enabled = true`.

### `GET /health`

Returns `{"status": "ok"}`.

### `GET /stats`

Returns rich JSON metrics including:

- 🖥️ **Server info** — version, uptime, adapter mode
- 🔗 **Connections** — current, total, total disconnections
- 📢 **Channels** — active channels per app
- 💬 **Messages** — WebSocket messages sent/received, HTTP calls
- 🌐 **Network** — bytes in/out (WebSocket and HTTP, broken down and totaled)
- ⚙️ **Process** — PID, RSS memory, virtual memory, peak memory, CPU user/system/total ms, thread count, open FDs, context switches
- 🖥️ **System** — CPU cores, load average (1m/5m/15m), total/available memory, system uptime
- 📱 **Per-app breakdown** — all metrics segmented by app ID

Stats are computed every 2 seconds in a background task. Process and system metrics are read via `spawn_blocking` to avoid blocking the async runtime.

---

## 🔴 Horizontal Scaling with Redis

For multi-node deployments, rushsocket uses Redis for cross-node coordination: pub/sub for event broadcasting, Hashes for global presence state, and key-value pairs for stats aggregation.

**1️⃣ Set up Redis** (version 6+ recommended).

**2️⃣ Update your config:**

```toml
adapter = "redis"

[redis]
url = "redis://your-redis-host:6379"
prefix = "rushsocket"
request_timeout = 5000
```

**3️⃣ Restart all rushsocket nodes.** Events triggered on any node are automatically broadcast to all connected clients across the cluster. The cache driver automatically uses Redis when the adapter is set to `"redis"`.

### 🗃️ Redis Data Model

rushsocket stores the following keys in Redis (all prefixed with the configured `prefix`):

| Key Pattern | Type | Purpose | TTL |
|---|---|---|---|
| `{prefix}:occupied:{app}:{channel}` | String | 🏠 Channel occupancy flag (SETNX for dedup) | None |
| `{prefix}:presence:{app}:{channel}` | Hash | 👥 Presence members: `user_id` -> `user_info JSON` | None |
| `{prefix}:psock:{app}:{channel}` | Hash | 🔢 Presence socket counts: `user_id` -> `count` | None |
| `{prefix}:node_stats:{node_id}` | String | 📊 Node stats JSON (connections, channels) | 15s |
| `{prefix}#broadcast` | Pub/Sub | 📡 Channel messages, heartbeats | — |
| `{prefix}#request` | Pub/Sub | 📨 Cross-node RPC requests | — |
| `{prefix}#response:{node_id}` | Pub/Sub | 📩 Cross-node RPC responses | — |

### 🔧 Cluster Features

**👥 Global Presence (Redis Hashes):** Presence members are stored in Redis Hashes, not local memory. When a user joins a presence channel, their info is written to Redis via `HSET`. When they leave (no sockets on any node), they're removed via `HDEL`. Queries like `get_presence_members` and `get_presence_user_count` read directly from Redis with `HGETALL` and `HLEN` — no cross-node broadcasting needed.

**📊 Node Stats Heartbeats:** Every 5 seconds, each node writes its local stats (per-app connection counts and channel socket counts) to a Redis key with a 15-second TTL. Stats queries (`get_connection_count`, `get_channels`, `get_channel_socket_count`) read fresh local data plus cached remote data via `MGET` — no cross-node broadcasting needed. If a node crashes, its stats expire in 15 seconds.

**🏠 Distributed Occupancy (SETNX/DEL):** `channel_occupied` and `channel_vacated` webhooks are deduplicated across the cluster. The first node to see a subscriber uses `SETNX` to claim the occupancy flag. The last node to see all subscribers leave verifies with a real-time cross-node query before deleting the flag and firing `channel_vacated`.

### ⚙️ Redis Adapter Behavior

- 🔍 **Peer discovery:** On startup (and after each reconnect), each node publishes an announce heartbeat and sends a discovery request to all peers. Peers respond, populating the node's peer registry. The `/ready` endpoint returns `503` until this completes.
- 💓 **Heartbeats:** Each node publishes a heartbeat every 30 seconds to keep connections alive and maintain the peer registry. Peers not seen for 60 seconds are pruned.
- 🔄 **Reconnect handling:** If the Redis subscriber connection drops, the node marks itself not-ready, reconnects, resubscribes, and re-discovers peers before accepting traffic again. A generation counter prevents stale discovery tasks from incorrectly restoring readiness.
- 🔀 **Connection splitting:** Broadcast traffic (channel messages, heartbeats) and control traffic (cross-node requests/responses) use separate Redis connections to prevent head-of-line blocking.
- 📏 **Per-node quotas:** Connection limits and presence member limits are enforced per-node, not cluster-wide. This is consistent with the Pusher protocol and avoids cross-node coordination on the hot path.
- ⏰ **Subscriber timeout:** If no message is received on the subscriber for 90 seconds (including heartbeats), the connection is assumed dead and a reconnect is forced.

---

## 🔒 TLS / SSL

rushsocket has built-in TLS support via **rustls** — no external reverse proxy needed.

**1️⃣ Obtain certificates** (e.g., via Let's Encrypt / certbot).

**2️⃣ Update your config:**

```toml
[ssl]
enabled = true
cert_path = "/etc/letsencrypt/live/yourdomain.com/fullchain.pem"
key_path  = "/etc/letsencrypt/live/yourdomain.com/privkey.pem"
```

**3️⃣ Update your client to use `wss://` and `forceTLS: true`.**

For certificate renewal, restart rushsocket after renewing to reload the new certificates.

---

## ☁️ Cloudflare (WSS Proxy)

You can use Cloudflare as a reverse proxy to handle TLS termination and DDoS protection — no need to configure TLS certificates on rushsocket itself.

### 🛠️ Setup

**1️⃣ Point your domain to your server** in Cloudflare DNS with the proxy enabled (orange cloud).

**2️⃣ Configure rushsocket to listen on a Cloudflare-compatible port.** Cloudflare only proxies WebSocket traffic on specific ports:

#### 🔐 Ports that support HTTPS/WSS (recommended)

| Port | Notes |
|---|---|
| `443` | Standard HTTPS |
| `2053` | |
| `2083` | |
| `2087` | |
| `2096` | |
| `8443` | Common alternative HTTPS |

#### 🔓 Ports that support HTTP/WS only

| Port | Notes |
|---|---|
| `80` | Standard HTTP |
| `8080` | |
| `8880` | |
| `2052` | |
| `2082` | |
| `2086` | |
| `2095` | |

For WSS (secure WebSocket), use one of the HTTPS ports. The recommended setup:

```toml
# config.toml — rushsocket listens on 443, Cloudflare handles TLS
port = 443

[ssl]
enabled = false   # Cloudflare terminates TLS, rushsocket receives plain WS
```

**3️⃣ Set Cloudflare SSL/TLS mode to "Flexible"** (Cloudflare encrypts client-to-edge, sends plain HTTP/WS to your origin) or **"Full"** if you also enable TLS on rushsocket.

**4️⃣ Update your client:**

```js
const pusher = new Pusher('your-app-key', {
    wsHost: 'ws.yourdomain.com',
    wsPort: 443,
    wssPort: 443,
    forceTLS: true,
    enabledTransports: ['ws', 'wss'],
    disableStats: true,
});
```

☁️ Cloudflare has a WebSocket idle timeout of 100 seconds. rushsocket's default `server_ping_interval` of 60 seconds keeps the connection alive well within this limit.

---

## 📢 Channel Types

rushsocket supports all Pusher channel types out of the box:

| Channel Prefix | Type | Description |
|---|---|---|
| *(none)* | 📢 **Public** | Anyone can subscribe, no authentication required |
| `private-` | 🔒 **Private** | Requires server-side authentication before subscribing |
| `presence-` | 👥 **Presence** | Private + tracks who is subscribed with user info |
| `private-encrypted-` | 🔐 **Encrypted Private** | End-to-end encrypted payload, server cannot read messages |
| `cache-` | 💾 **Cache** | Replays the last event to new subscribers (combinable: `cache-private-`, `cache-presence-`) |

---

## 🏗️ Architecture

### 🔄 Connection Lifecycle

1. 🔌 Client connects via WebSocket at `/app/{key}`
2. ✅ Server validates the app key, checks connection limits (atomic CAS) and memory threshold (cached, non-blocking)
3. 📨 On success, sends `pusher:connection_established` with a unique `socket_id`
4. 🏓 Server sends periodic pings; client must respond with pongs within the configured timeout
5. 🐢 Slow consumers (clients whose outbound buffer is full) are automatically force-closed

### 🔌 Adapter Model

rushsocket uses an adapter pattern to abstract single-node vs. multi-node operation:

- **Local adapter:** All state is in-memory using lock-free `DashMap` structures. Suitable for single-node deployments.
- **Redis adapter:** Wraps the local adapter and adds Redis for cross-node coordination. Channel messages are broadcast via pub/sub. Presence state is stored in Redis Hashes. Stats are exchanged via periodic heartbeat keys with TTL. A request/response protocol handles cluster-wide queries that can't be served from Redis alone (e.g., listing socket IDs in a channel, aggregating metrics).

### 🧠 Key Design Decisions

- ⚛️ **Atomic admission control:** Connection limits use `AtomicUsize` with CAS loops — no TOCTOU races between checking the limit and inserting the socket.
- ⚛️ **Atomic presence limits:** Presence member limits are checked and enforced while holding the DashMap entry guard, preventing over-admission races.
- 📋 **Zero-copy fan-out:** Channel messages are serialized once into `bytes::Bytes` (reference-counted), then shared across all recipients without per-socket copies.
- 🧠 **Non-blocking memory checks:** Memory usage is sampled every 2 seconds by a background task and cached in an `AtomicU64`. The WebSocket handshake reads a single atomic — no `/proc` I/O on the hot path.
- ⚡ **Non-blocking webhooks:** Webhook events are queued via an unbounded mpsc channel. The WebSocket handler never waits for HTTP delivery.
- 🔄 **Generation-guarded readiness:** Redis reconnects bump an atomic generation counter. Stale peer discovery tasks from previous connections cannot flip the readiness flag.
- 🔐 **Lock safety:** Per-connection state uses `parking_lot::Mutex` which never poisons. A panicked thread cannot cascade into server-wide lock failures.
- 🛡️ **Body integrity:** POST requests with a `body_md5` query parameter are verified against the actual request body, catching tampering or corruption in transit.

---

## 📁 Project Structure

```
src/
├── adapter/         # 🔌 Storage abstraction layer
│   ├── mod.rs       #   Adapter trait definition
│   ├── local.rs     #   Single-node in-memory adapter
│   ├── namespace.rs #   Per-app state (sockets, channels, presence)
│   └── redis/       #   Multi-node Redis adapter
│       ├── mod.rs   #     Redis adapter (presence hashes, stats heartbeats, occupancy)
│       ├── pubsub.rs#     Pub/sub, heartbeats, peer discovery
│       └── request_response.rs # Cross-node RPC types
├── app/             # 📱 App management and lookup
├── auth/            # 🔑 HMAC-SHA256 channel and signin authentication
├── cache/           # 💾 Cache drivers (memory + Redis)
├── config/          # ⚙️ Configuration loading and defaults
├── http/            # 🌐 Pusher HTTP REST API
│   ├── auth.rs      #   HTTP request signature and body MD5 verification
│   ├── mod.rs       #   Router setup
│   └── routes/      #   Endpoint handlers
├── metrics/         # 📊 Atomic counters for real-time stats
├── protocol/        # 📡 Pusher protocol messages, channel types, error codes
├── rate_limit/      # 🛡️ Per-app rate limiting
├── webhook/         # 🪝 Webhook delivery (batching, HMAC signing, retries)
├── websocket/       # 🔌 WebSocket handler and event processing
│   ├── mod.rs       #   Connection handler, admission control
│   ├── connection.rs#   Per-connection state (parking_lot::Mutex)
│   ├── lifecycle.rs #   Message routing and disconnect cleanup
│   ├── socket_id.rs #   Socket ID generation
│   └── events/      #   Event handlers (subscribe, unsubscribe, signin, client events, ping/pong)
├── server.rs        # 🚀 Server orchestration, startup, sd-notify, stats computation
├── state.rs         # 🏗️ Shared application state
├── util.rs          # 🧠 Memory usage sampling (Linux /proc, Windows Win32)
├── lib.rs           # 📦 Module exports
└── main.rs          # 🏁 CLI entry point
```

---

## 🔌 Default Ports Reference

| Port | Purpose |
|---|---|
| `6001` | 🔌 WebSocket connections + HTTP REST API + health/readiness endpoints |
| `9100` | 📊 Stats (`/stats`) and health check (`/health`) — only when `metrics_enabled = true` |

---

## 🤝 Contributing

Contributions are welcome. Please open an issue or pull request.

---

## 📜 License

rushsocket is dual-licensed under [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE).

---

## 🔗 Links

- 🌐 **Website:** [rushsocket.xyz](https://rushsocket.xyz)
- 📧 **Email:** [dev@rushsocket.xyz](mailto:dev@rushsocket.xyz)
- 🐙 **GitHub:** [github.com/rushsocket/rushsocket](https://github.com/rushsocket/rushsocket)
