#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked
mkdir -p bin
cp "${CARGO_TARGET_DIR:-target}/release/herdr-git-graph" bin/herdr-git-graph
chmod +x bin/herdr-git-graph
