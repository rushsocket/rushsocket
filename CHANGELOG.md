# Changelog

## Unreleased

## 1.2.0 - 2026-11-28

**Redis upgrade:** Drain and stop all v1.1.0 or earlier nodes before starting v1.2.0. The presence key format and termination protocol have changed. Clients rebuild presence when they reconnect. Preserve Redis cache keys.

- Fixed the built-in TLS listener abort when both rustls crypto providers are linked. Updated the locked dependencies.
- Redis connections recover after a restart. Readiness waits for peer discovery and state restoration. Crashed nodes’ presence rosters expire and concurrent joins protect the occupancy claim.
- Cluster termination waits for peer acknowledgements. A node that loses its revocation subscription closes signed-in sockets. Incomplete termination or event publication returns HTTP 503.
- Bounded WebSocket reads, writes, webhook backlog and delivery concurrency. Queued webhooks can drain during shutdown. HTTP API routes share the configured concurrency limit.
- Protected generated credentials, removed sensitive URLs from logs, and fixed systemd startup notification and installer restarts on upgrade.

## 1.1.0 - 2026-08-23

### Performance
- Channel fan-out snapshots connection handles and drops the DashMap shard guard before `send_raw`.
- Redis adapter commands clone the multiplexed connection instead of holding a mutex across RTT; publish reconnect is a generation slot-swap.
- Redis subscriber loop is parse+dispatch only: incoming RPC responses stay on the reader, broadcasts go to a bounded worker, occupancy replies are unbounded.
- `GET /channels` and REST `info=subscription_count` use one `node_stats` MGET for the count pass, not one per channel.
- Webhook queue is bounded (4096); HTTP retries run off the batch loop. `/stats` reports `drops.pubsub_broadcast` and `drops.webhook_events`.

### Fixed
- Cross-node vacate RPC count replies serialize as `Count { count }` so peers can deserialize the integer (internally tagged `Count(usize)` was `{"type":"Count"}` with no value, so vacate saw `received=0`).
- REST `auth_timestamp` window is symmetric `|now-ts|` (future timestamps no longer replay forever).
- Invalid `metrics_host` (e.g. `localhost`) fails config load instead of inheriting the public WS bind.
- Webhook HTTP client does not follow redirects (HMAC headers stay on the configured URL).
- Webhook worker flushes on interval under load and at a batch cap of 10; `webhook_batch_ms` must be >= 1.
- `/batch_events` enforces `max_event_name_length` like `/events`.
- Redis connection admission: EVAL errors on unlimited apps admit with a warning; finite apps get `pusher:error` 4009 `"Connection unavailable"` (not 4004).
- Vacate RPC timeout/partial/`Err` no longer treats missing peers as zero subscribers (no false `channel_vacated`).
- Occupancy keys use `SET NX EX 15` and are refreshed by every local occupant from the stats heartbeat.
- Presence `channel_occupied` uses the same global SETNX path as public/private.
- Rate-limit values above `u32::MAX` clamp instead of aborting the process.
- Repeat `pusher:signin` with a different user id removes the previous user→socket mapping.
- `install.sh` refuses Darwin (and non-Linux) before architecture detection; Intel macOS no longer downloads a Linux binary.
- GET `/channels/{name}` honors `info` (default occupied-only).
- REST HMAC still signs decoded query values; percent-encoded `@`/`,` on the wire verify.
- Handle `pusher:pong` without emitting `pusher:error` 4009.
- Recognize Pusher v7 cache prefixes (`private-cache-`, `presence-cache-`, `private-encrypted-cache-`). Legacy `cache-private-*` names still work as **separate** channel names.
- Per-channel subscribe failures now emit `pusher:subscription_error` instead of a connection-level 4009.
- Connection quota rejection uses close/error code **4004** (do not reconnect immediately) instead of 4100.
- Re-subscribe on an already-joined channel re-sends `subscription_succeeded`.
- Client events are rejected on public and encrypted channels; unknown `client_event_mode` is fail-closed.
- REST HMAC requires `auth_timestamp` (default 600s window), `auth_version=1.0`, matching `auth_key`, `body_md5` on nonempty POST, `app.enabled`, and constant-time compare.
- Empty `channels: []` and invalid channel names return HTTP 400 on trigger.
- Graceful shutdown wakes connections via `watch` and sends `pusher:error` 4200; READY waits for Redis discovery (`from_millis(max(2*request_timeout, request_timeout+2000))`); WS handshake is 503 when not ready or closing.
- Redis presence add/remove is one Lua eval each (live TOCTOU only). Cluster connection quota is a per-app HASH SET-absolute with a per-app mutex; `GET /connections` reads the same HASH; crashed-node fields are reaped when `node_stats` expires.
- `RedisCache` applies `redis.prefix` inside the driver (`{prefix}:cache:{app}:{channel}`). **Breaking:** unprefixed keys miss until TTL.
- Config load fails on unknown keys (e.g. `ssl.enable` typo), unknown `adapter` / `client_event_mode`, duplicate app id/key, `ssl.enabled` without paths, and empty `[[apps]]`.
- Default bind is `127.0.0.1`; metrics bind separately (`metrics_host`). `install.sh` generates key/secret. Supported platforms: Linux and macOS only (Windows is not supported).

### Removed
- Unused ping/pong constructors, dead metrics aggregation RPC, `GetChannelSockets` self-RPC, unused cache `forget`, unused occupancy/metrics helpers, unused crates (`axum-extra`, `hyper`, `url`, `tokio-stream`, `async-stream`).

Redis integration tests that need a server skip unless `REDIS_URL` is set (CI: `REDIS_URL=redis://127.0.0.1:6379 cargo test`).
