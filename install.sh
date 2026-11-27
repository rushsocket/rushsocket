#!/usr/bin/env bash
set -euo pipefail

REPO="rushsocket/rushsocket"
BINARY_NAME="rushsocket"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/rushsocket"
SERVICE_FILE="/etc/systemd/system/rushsocket.service"

echo "=== rushsocket installer ==="
echo ""

# 1. Detect OS then architecture. Pre-built binaries are Linux-only.
OS="$(uname -s)"
if [ "$OS" = "Darwin" ]; then
    echo "Error: pre-built binaries are Linux-only."
    echo "On macOS, build from source:"
    echo "  git clone https://github.com/${REPO}.git"
    echo "  cd rushsocket && cargo build --release"
    exit 1
fi
if [ "$OS" != "Linux" ]; then
    echo "Error: unsupported OS: ${OS}"
    echo "Supported platforms: Linux (pre-built) and macOS (build from source). Windows is not supported."
    echo "Build from source:"
    echo "  git clone https://github.com/${REPO}.git"
    echo "  cd rushsocket && cargo build --release"
    exit 1
fi

ARCH="$(uname -m)"
case "$ARCH" in
    x86_64)  ASSET="rushsocket-linux-x86_64" ;;
    aarch64) ASSET="rushsocket-linux-aarch64" ;;
    *)
        echo "Error: no pre-built binary for architecture: ${ARCH}"
        echo "Build from source instead:"
        echo "  git clone https://github.com/${REPO}.git"
        echo "  cd rushsocket && cargo build --release"
        exit 1
        ;;
esac

# 2. Resolve version (use argument or fetch latest)
if [ "${1:-}" != "" ]; then
    VERSION="$1"
else
    echo "Fetching latest version..."
    VERSION="$(curl -sSf "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | cut -d '"' -f 4)"
fi
echo "Version: ${VERSION}  Arch: ${ARCH}"
echo ""

# 3. Download and verify
TARBALL="${ASSET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${TARBALL}"
SHA_URL="${URL}.sha256"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Downloading ${TARBALL}..."
curl -sSfL -o "${TMPDIR}/${TARBALL}" "$URL"
curl -sSfL -o "${TMPDIR}/${TARBALL}.sha256" "$SHA_URL"

echo "Verifying checksum..."
(cd "$TMPDIR" && sha256sum -c "${TARBALL}.sha256")

# 4. Install binary
echo "Installing binary to ${INSTALL_DIR}/${BINARY_NAME}..."
tar xzf "${TMPDIR}/${TARBALL}" -C "$TMPDIR"
install -m 755 "${TMPDIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"

# 5. Install default config (preserve existing)
mkdir -p "$CONFIG_DIR"
if [ ! -f "${CONFIG_DIR}/config.toml" ]; then
    echo "Installing default config to ${CONFIG_DIR}/config.toml..."
    GEN_SECRET="$(openssl rand -hex 16)"
    GEN_KEY="$(openssl rand -hex 8)"
    cat > "${TMPDIR}/config.toml" << TOML
# rushsocket configuration (Linux). macOS: cargo run; Windows is not supported.
# Bind 0.0.0.0 only if you intend to listen on all interfaces.

host = "127.0.0.1"
port = 6001
metrics_host = "127.0.0.1"

[ssl]
enabled = false

[[apps]]
id = "rushsocket-id"
key = "${GEN_KEY}"
secret = "${GEN_SECRET}"
TOML
    install -m 600 "${TMPDIR}/config.toml" "${CONFIG_DIR}/config.toml"
else
    echo "Config already exists at ${CONFIG_DIR}/config.toml — skipping."
fi

chmod go-rwx "${CONFIG_DIR}/config.toml"

# 6. Install systemd service
echo "Installing systemd service..."
cat > "$SERVICE_FILE" << 'UNIT'
[Unit]
Description=rushsocket - Pusher Protocol v7 WebSocket Server
After=network-online.target

[Service]
Type=notify
ExecStart=/usr/local/bin/rushsocket --config /etc/rushsocket/config.toml
Restart=always
RestartSec=3
TimeoutStopSec=30
# default request_timeout=5000ms → READY wait cap 10s; TimeoutStartSec = max(30, wait_cap_secs+10) = 30
TimeoutStartSec=30
LimitNOFILE=1000000
TasksMax=infinity
Nice=-10
LimitCORE=0

[Install]
WantedBy=multi-user.target
UNIT

# 7. Enable and start
echo "Enabling and starting rushsocket..."
systemctl daemon-reload
systemctl enable rushsocket
# Type=notify waits for the new process to bind and finish adapter discovery.
systemctl restart rushsocket

# Read ports from config (fall back to defaults)
WS_PORT=$(grep -E '^\s*port\s*=' "${CONFIG_DIR}/config.toml" 2>/dev/null | head -1 | sed 's/.*=\s*//' | tr -d ' "' || true)
METRICS_PORT=$(grep -E '^\s*metrics_port\s*=' "${CONFIG_DIR}/config.toml" 2>/dev/null | head -1 | sed 's/.*=\s*//' | tr -d ' "' || true)
WS_PORT="${WS_PORT:-6001}"
METRICS_PORT="${METRICS_PORT:-9100}"

echo ""
echo "=== rushsocket ${VERSION} installed ==="
echo ""
echo "  Status:  systemctl status rushsocket"
echo "  Logs:    journalctl -u rushsocket -f"
echo "  Config:  ${CONFIG_DIR}/config.toml"
echo "  WS:      ws://localhost:${WS_PORT}/app/{key}"
echo "  Stats:   curl http://localhost:${METRICS_PORT}/stats"
echo "  Health:  curl http://localhost:${METRICS_PORT}/health"
echo ""
