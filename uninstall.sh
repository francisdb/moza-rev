#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="moza-rev"
INSTALL_DIR="/usr/local/bin"
SERVICE_DIR="/etc/systemd/system"

HAS_SERVICE=false
HAS_BINARY=false

if systemctl is-enabled "$SERVICE_NAME" &>/dev/null || [ -f "$SERVICE_DIR/$SERVICE_NAME.service" ]; then
    HAS_SERVICE=true
fi
if [ -f "$INSTALL_DIR/$SERVICE_NAME" ]; then
    HAS_BINARY=true
fi

if [ "$HAS_SERVICE" = false ] && [ "$HAS_BINARY" = false ]; then
    echo "Service is not running."
    echo "Service is not installed."
    echo "Binary is not installed."
    exit 0
fi

# Ensure we have root privileges
if [ "$EUID" -ne 0 ]; then
    echo "Requesting elevated privileges..."
    exec sudo "$0" "$@"
fi

if [ "$HAS_SERVICE" = true ]; then
    echo "Stopping and disabling service..."
    systemctl disable --now "$SERVICE_NAME" 2>/dev/null || true
    echo "Removing service file..."
    rm -f "$SERVICE_DIR/$SERVICE_NAME.service"
    systemctl daemon-reload
else
    echo "Service is not installed, skipping."
fi

if [ "$HAS_BINARY" = true ]; then
    echo "Removing binary..."
    rm -f "$INSTALL_DIR/$SERVICE_NAME"
else
    echo "Binary is not installed, skipping."
fi

echo "Done! Service has been removed."
