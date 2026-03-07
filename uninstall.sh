#!/usr/bin/env bash
set -euo pipefail

BINARY_NAME="rushsocket"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/rushsocket"
SERVICE_FILE="/etc/systemd/system/rushsocket.service"

echo "=== rushsocket uninstaller ==="
echo ""

# 1. Stop and disable service
if systemctl is-active --quiet rushsocket 2>/dev/null; then
    echo "Stopping rushsocket..."
    systemctl stop rushsocket
fi

if systemctl is-enabled --quiet rushsocket 2>/dev/null; then
    echo "Disabling rushsocket..."
    systemctl disable rushsocket
fi

# 2. Remove service file
if [ -f "$SERVICE_FILE" ]; then
    echo "Removing service file..."
    rm -f "$SERVICE_FILE"
    systemctl daemon-reload
fi

# 3. Remove binary
if [ -f "${INSTALL_DIR}/${BINARY_NAME}" ]; then
    echo "Removing binary..."
    rm -f "${INSTALL_DIR}/${BINARY_NAME}"
fi

# 4. Remove config (--purge flag) or preserve
if [ -d "$CONFIG_DIR" ]; then
    if [ "${1:-}" = "--purge" ]; then
        echo "Removing config directory..."
        rm -rf "$CONFIG_DIR"
        echo "Config removed."
    else
        echo "Config preserved at ${CONFIG_DIR}."
        echo "Run with --purge to also remove config."
    fi
fi

echo ""
echo "=== rushsocket uninstalled ==="
