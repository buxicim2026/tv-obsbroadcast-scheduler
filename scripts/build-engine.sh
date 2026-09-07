#!/usr/bin/env bash
# build-engine.sh — Build the Rust engine binary.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> Building Rust engine (release) ..."
cargo build --release --manifest-path "$ROOT/engine/Cargo.toml"
echo "==> Done. Binary at engine/target/release/tv-obsbroadcast-scheduler"
