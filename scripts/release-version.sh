#!/bin/sh
# 日本時間の日付と既存タグから次のリリースタグを決める。引数があれば形式を検証する。
# usage: scripts/release-version.sh [YYYY-MM-dd[.N]]
set -eu
cd "$(dirname "$0")/.."

python3 - "$@" <<'PY'
import re
import subprocess
import sys
from datetime import date, datetime
from zoneinfo import ZoneInfo


def valid_tag(tag):
    if not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}(\.[1-9][0-9]*)?", tag):
        return False
    try:
        date.fromisoformat(tag.split(".")[0])
    except ValueError:
        return False
    return True


if len(sys.argv) > 2:
    sys.exit("Usage: scripts/release-version.sh [YYYY-MM-dd[.N]]")
if len(sys.argv) == 2:
    tag = sys.argv[1]
    if not valid_tag(tag):
        sys.exit(f"Invalid release tag '{tag}': expected YYYY-MM-dd or YYYY-MM-dd.N (N >= 1).")
    print(tag)
else:
    today = datetime.now(ZoneInfo("Asia/Tokyo")).date().isoformat()
    tags = subprocess.check_output(["git", "tag", "--list", f"{today}*"], text=True)
    suffixes = [
        int(tag.split(".")[1]) if "." in tag else 0
        for tag in tags.splitlines()
        if valid_tag(tag)
    ]
    print(f"{today}.{max(suffixes) + 1}" if suffixes else today)
PY
