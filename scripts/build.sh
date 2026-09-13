#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null 2>&1; then
    echo 'Source builds require Rust and Cargo (https://rustup.rs). For a prebuilt binary, run: sh scripts/install.sh' >&2
    exit 1
fi
case "${1:---release}" in
    --debug) profile=debug; cargo build --locked ;;
    --release) profile=release; cargo build --release --locked ;;
    *) echo 'Usage: sh scripts/build.sh [--debug|--release]' >&2; exit 2 ;;
esac
mkdir -p bin
temporary=$(mktemp bin/.herdr-git-graph.XXXXXX)
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cp "${CARGO_TARGET_DIR:-target}/$profile/herdr-git-graph" "$temporary"
chmod +x "$temporary"
mv -f "$temporary" bin/herdr-git-graph
