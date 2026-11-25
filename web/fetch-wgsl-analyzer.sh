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

VERSION=0.12.0-next.542
INTEGRITY=sha512-qhoaLrW5KVx9brijlfHPhqPt+f4fyhgwTj/XzVuOACfM5hxXgfzwh/5a9OwrkTbj2v1ICz4wfoYW+6VOyKgo4A==
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

# Plain ES modules since 0.12.0-next.5xx: index.js spawns worker.js, which
# imports host.js (and it fs.js) and loads the emscripten glue and wasm next
# to itself. The rest of dist/ (types, source maps, protocol.js and
# emscripten.js, imported for types only) is not needed at runtime.
rm -rf "$DEST"
mkdir -p "$DEST"
for file in index.js worker.js host.js fs.js wgsl_analyzer.js wgsl_analyzer.wasm; do
    cp "$TMP/package/dist/$file" "$DEST/"
done
cp "$TMP/package/LICENSE-MIT" "$TMP/package/LICENSE-APACHE" "$DEST/"
echo "$VERSION" >"$DEST/VERSION"
