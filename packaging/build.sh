#!/usr/bin/env bash
# Build and stage a native Linux executable for native-packages.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

case "${1:-$(uname -m)}" in
    amd64|x86_64) arch=amd64; target=x86_64-unknown-linux-gnu ;;
    arm64|aarch64) arch=arm64; target=aarch64-unknown-linux-gnu ;;
    *) echo "Usage: $0 [amd64|arm64]" >&2; exit 1 ;;
esac

# Cross builds require a suitable Rust target, linker and libudev sysroot.
# CI builds each architecture on a native runner.
cargo build --locked --release --bin moza-rev --target "$target"
install -Dm755 "${CARGO_TARGET_DIR:-target}/$target/release/moza-rev" "dist/linux-$arch/moza-rev"
