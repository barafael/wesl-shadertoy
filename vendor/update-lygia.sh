#!/bin/sh
# Refresh vendor/lygia from upstream: the WESL modules of LYGIA
# (https://github.com/patriciogonzalezvivo/lygia) plus its license, at the
# commit given as the first argument (default: the upstream main branch).
#
# Only `*.wesl` files are kept (the package wesl.toml declares, minus the
# test/ tree); crates/core/build.rs embeds them as the `lygia` WESL package.
set -eu

repo=https://github.com/patriciogonzalezvivo/lygia.git
rev=${1:-main}
here=$(cd "$(dirname "$0")" && pwd)
dest=$here/lygia
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

git -C "$tmp" init -q
git -C "$tmp" fetch -q --depth 1 "$repo" "$rev"
git -C "$tmp" checkout -q FETCH_HEAD
commit=$(git -C "$tmp" rev-parse HEAD)

rm -rf "$dest"
mkdir -p "$dest"
(cd "$tmp" && find . -name '*.wesl' -not -path './test/*' -not -path './node_modules/*') |
    while read -r file; do
        mkdir -p "$dest/$(dirname "$file")"
        cp "$tmp/$file" "$dest/$file"
    done
cp "$tmp/LICENSE.md" "$dest/LICENSE.md"
cat >"$dest/VENDORED.md" <<EOF
# LYGIA (vendored subset)

The \`.wesl\` modules of [LYGIA](https://github.com/patriciogonzalezvivo/lygia)
by Patricio Gonzalez Vivo and contributors, at commit
\`$commit\`, copied by \`vendor/update-lygia.sh\`.

Licensed under the Prosperity Public License 3.0.0 (see \`LICENSE.md\`) or the
Patron License (https://lygia.xyz/license); individual files may carry
additional notices for the third-party code they include. This directory is
**not** covered by wesl-shadertoy's MIT OR Apache-2.0 license.
EOF
echo "vendored $(find "$dest" -name '*.wesl' | wc -l) modules from $commit"
