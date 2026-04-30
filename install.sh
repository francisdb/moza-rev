#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="moza-rev"
INSTALL_DIR="/usr/local/bin"
SERVICE_DIR="/etc/systemd/system"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Check for cargo
if ! command -v cargo &>/dev/null; then
    echo "Error: cargo is not installed. Install Rust via https://rustup.rs/"
    exit 1
fi

# Detect target user — if invoked via sudo, use the original caller. The
# service runs as a normal user (with serial access via the dialout group),
# not as root.
TARGET_USER="${SUDO_USER:-$USER}"
if [ "$TARGET_USER" = "root" ]; then
    echo "Error: refusing to install with target user 'root'. Run this script as your"
    echo "       normal user; it will request sudo for the system-level steps itself."
    exit 1
fi

# Verify user is in dialout group (needed for /dev/ttyACM* access). Warn but
# don't bail — user may have arranged access via a different group / udev rule.
if ! id -nG "$TARGET_USER" | grep -qw dialout; then
    echo "Warning: user '$TARGET_USER' is not in the 'dialout' group."
    echo "         Add with: sudo usermod -aG dialout $TARGET_USER  (then re-login)"
fi

# Build as current user (before sudo)
echo "Building release binary..."
cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"

# Ensure we have root privileges for install
if [ "$EUID" -ne 0 ]; then
    echo "Requesting elevated privileges..."
    NEED_SUDO=sudo
else
    NEED_SUDO=
fi

# Stop service if running (binary can't be overwritten while in use)
$NEED_SUDO systemctl stop "$SERVICE_NAME" 2>/dev/null || true

# Install binary
echo "Installing binary to $INSTALL_DIR..."
$NEED_SUDO install -m 0755 "$SCRIPT_DIR/target/release/$SERVICE_NAME" "$INSTALL_DIR/$SERVICE_NAME"

# Render service file (substitute @USER@) and install. Using a here-pipeline
# rather than touching the source tree means the committed unit stays a
# template and you can re-run install.sh with a different user trivially.
echo "Installing systemd service (User=$TARGET_USER)..."
sed "s/@USER@/$TARGET_USER/" "$SCRIPT_DIR/$SERVICE_NAME.service" \
    | $NEED_SUDO tee "$SERVICE_DIR/$SERVICE_NAME.service" > /dev/null

$NEED_SUDO systemctl daemon-reload
$NEED_SUDO systemctl enable "$SERVICE_NAME"
$NEED_SUDO systemctl restart "$SERVICE_NAME"

echo "Done! Service is running."
echo "  Status:  systemctl status $SERVICE_NAME"
echo "  Logs:    journalctl -u $SERVICE_NAME -f"
echo "  Stop:    sudo systemctl disable --now $SERVICE_NAME"
