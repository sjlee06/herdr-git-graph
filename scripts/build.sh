#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null 2>&1; then
    echo 'Source builds require Rust and Cargo (https://rustup.rs). For a prebuilt binary, run: sh scripts/install.sh' >&2
    exit 1
fi
cargo build --release --locked
mkdir -p bin
cp "${CARGO_TARGET_DIR:-target}/release/herdr-git-graph" bin/herdr-git-graph
chmod +x bin/herdr-git-graph
