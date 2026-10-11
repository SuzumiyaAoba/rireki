#!/bin/sh
# Cargo.toml のバージョンとリリース指定を照合し、タグ名を出力する。
# usage: scripts/release-version.sh [version-or-tag]
set -eu
cd "$(dirname "$0")/.."

version=$(awk '
    /^\[package\]$/ { package = 1; next }
    /^\[/ { package = 0 }
    package && /^version = "/ { split($0, fields, "\""); print fields[2]; exit }
' Cargo.toml)

if [ -z "$version" ]; then
    echo 'Cannot read the package version from Cargo.toml.' >&2
    exit 1
fi

requested=${1:-$version}
if [ "${requested#v}" != "$version" ]; then
    echo "Release version '$requested' does not match Cargo.toml version '$version'." >&2
    exit 1
fi

printf 'v%s\n' "$version"
