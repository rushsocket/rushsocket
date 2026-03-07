#!/usr/bin/env bash
set -euo pipefail

REPO="rushsocket/rushsocket"
BINARY_NAME="rushsocket"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/rushsocket"
SERVICE_FILE="/etc/systemd/system/rushsocket.service"

echo "=== rushsocket installer ==="
echo ""

# 1. Detect architecture
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
    cat > "${CONFIG_DIR}/config.toml" << 'TOML'
# rushsocket configuration
# See https://github.com/rushsocket/rushsocket for the full reference.

# host = "0.0.0.0"
# port = 6001

[ssl]
enabled = false

[[apps]]
id = "rushsocket-id"
key = "rushsocket-key"
secret = "rushsocket-secret"
TOML
else
    echo "Config already exists at ${CONFIG_DIR}/config.toml — skipping."
fi

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
TimeoutStopSec=15
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
systemctl enable --now rushsocket

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
