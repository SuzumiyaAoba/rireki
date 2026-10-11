#!/bin/sh
# リリースビルドから target/release/rireki.app を組み立てる。
# usage: scripts/bundle.sh
set -eu
cd "$(dirname "$0")/.."

cargo build --release --locked

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
app=target/release/rireki.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/rireki "$app/Contents/MacOS/rireki"
cp assets/app-icon/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
sed "s/__VERSION__/$version/g" assets/app-icon/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null
# Finder と Dock のアイコンのキャッシュを更新する
touch "$app"
echo "$app"
