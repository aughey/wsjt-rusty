#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  pkg-config libasound2-dev libudev-dev libsqlite3-dev \
  libx11-dev libxi-dev libxrandr-dev libxcursor-dev libxinerama-dev \
  libxkbcommon-dev libwayland-dev libgl1-mesa-dev
cargo build -p wsjt-native --release --locked
cargo test --workspace --release --locked
# Detect missing runtime dependencies without starting devices or the GUI.
ldd target/release/wsjt-native
if ldd target/release/wsjt-native | grep -q 'not found'; then
  exit 1
fi
