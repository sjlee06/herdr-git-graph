#!/bin/sh
# Install the release matching this checkout; Rust is only needed for source builds.
set -eu
cd "$(dirname "$0")/.."

fail() { printf '%s\n' "herdr-git-graph: $*" >&2; exit 1; }
if [ "${HERDR_GIT_GRAPH_BUILD_FROM_SOURCE:-0}" = 1 ]; then
    exec sh scripts/build.sh
fi

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64|Darwin/aarch64) target=aarch64-apple-darwin ;;
    Darwin/x86_64) target=x86_64-apple-darwin ;;
    Linux/aarch64|Linux/arm64) target=aarch64-unknown-linux-gnu ;;
    Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
    *) fail "No prebuilt binary for $(uname -s)/$(uname -m). Install Rust from https://rustup.rs and run sh scripts/build.sh." ;;
esac
command -v curl >/dev/null 2>&1 || fail "curl is required to download the release."
if command -v sha256sum >/dev/null 2>&1; then
    checksum=sha256sum
elif command -v shasum >/dev/null 2>&1; then
    checksum=shasum
else
    fail "Install sha256sum or shasum to verify the download."
fi
version=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' herdr-plugin.toml)
[ -n "$version" ] || fail "Cannot read the release version from herdr-plugin.toml."
asset="herdr-git-graph-$target"
url="https://github.com/sjlee06/herdr-git-graph/releases/download/v$version/$asset"
mkdir -p bin
stage=$(mktemp -d ./bin/.install.XXXXXX)
trap 'rm -rf "$stage"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
printf 'Installing herdr-git-graph v%s (%s)...\n' "$version" "$target"
for suffix in '' .sha256; do
    curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
        --retry 2 --connect-timeout 15 --max-time 180 \
        "$url$suffix" --output "$stage/$asset$suffix" || \
        fail "Release download failed. Check your connection and retry. To build locally, install Rust and run sh scripts/build.sh."
done
expected=$(awk 'NR == 1 {print $1}' "$stage/$asset.sha256")
case "$expected" in ''|*[!0-9a-fA-F]*) fail "Invalid release checksum." ;; esac
[ "${#expected}" -eq 64 ] || fail "Invalid release checksum."
if [ "$checksum" = sha256sum ]; then
    actual=$(sha256sum "$stage/$asset" | awk '{print $1}')
else
    actual=$(shasum -a 256 "$stage/$asset" | awk '{print $1}')
fi
[ "$expected" = "$actual" ] || fail "Checksum mismatch; the existing installation was not replaced."
chmod +x "$stage/$asset"
"$stage/$asset" --version || fail "This release cannot run on your OS. See README requirements or build locally with Rust."
mv -f "$stage/$asset" bin/herdr-git-graph
printf '%s\n' 'Installed bin/herdr-git-graph (Rust is not required).'
