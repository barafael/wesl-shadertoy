#!/bin/sh
# Fetches the pinned wgsl-analyzer-web build (the wgsl-analyzer language
# server compiled to wasm; MIT OR Apache-2.0) into web/vendor/wgsl-analyzer,
# which index.html copies into the trunk output as `wgsl-analyzer/`.
#
# Trunk.toml runs this as a pre_build hook. It is a no-op once the pinned
# version is in place, and checks the npm integrity hash before unpacking.
# Set WESL_SHADERTOY_NO_ANALYZER=1 to build offline without the analyzer
# (the app then reports it as unavailable).
set -eu

VERSION=0.12.0-next.510
INTEGRITY=sha512-1IugGZE1BtaBmgpf0Bl7P2LrwN1JbxeogbPcgOczDI3XSBp/QEM1Ve6M6Hr2SetzrdXf1ERnVbuoNyvE8dwQMA==
URL="https://registry.npmjs.org/wgsl-analyzer-web/-/wgsl-analyzer-web-$VERSION.tgz"

cd "$(dirname "$0")"
DEST=vendor/wgsl-analyzer

if [ "${WESL_SHADERTOY_NO_ANALYZER:-}" = 1 ]; then
    mkdir -p "$DEST"
    exit 0
fi
if [ "$(cat "$DEST/VERSION" 2>/dev/null || true)" = "$VERSION" ]; then
    exit 0
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
echo "fetching wgsl-analyzer-web $VERSION" >&2
curl -fsSL "$URL" -o "$TMP/package.tgz"
GOT="sha512-$(openssl dgst -sha512 -binary "$TMP/package.tgz" | openssl base64 -A)"
if [ "$GOT" != "$INTEGRITY" ]; then
    echo "wgsl-analyzer-web $VERSION: integrity mismatch (got $GOT)" >&2
    exit 1
fi
tar -xzf "$TMP/package.tgz" -C "$TMP"

# index.js spawns assets/worker.js, which loads the emscripten glue and wasm
# next to itself; nothing else in the package is needed at runtime.
rm -rf "$DEST"
mkdir -p "$DEST/assets"
cp "$TMP/package/dist/index.js" "$TMP/package/LICENSE-MIT" "$TMP/package/LICENSE-APACHE" "$DEST/"
cp "$TMP/package/dist/assets/worker.js" \
    "$TMP/package/dist/assets/wgsl_analyzer.js" \
    "$TMP/package/dist/assets/wgsl_analyzer.wasm" \
    "$DEST/assets/"
echo "$VERSION" >"$DEST/VERSION"
