#!/bin/sh
# アプリと Applications へのリンクを含む配布用 DMG を作る。
# usage: scripts/dmg.sh dist/rireki-<tag>-<target>.dmg
set -eu
cd "$(dirname "$0")/.."

if [ "$#" -ne 1 ]; then
    echo 'Usage: scripts/dmg.sh <output.dmg>' >&2
    exit 1
fi

app=target/release/rireki.app
if [ ! -d "$app" ]; then
    echo 'App bundle not found. Run scripts/bundle.sh first.' >&2
    exit 1
fi

output=$1
mkdir -p "$(dirname "$output")"
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT HUP INT TERM
ditto "$app" "$staging/rireki.app"
ln -s /Applications "$staging/Applications"
hdiutil create -volname rireki -srcfolder "$staging" -format UDZO -fs HFS+ "$output"
hdiutil verify "$output"
