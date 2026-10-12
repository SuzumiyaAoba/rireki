#!/usr/bin/env python3
"""日付タグの採番と、再ビルド時のタグ検証を一時リポジトリで確認する。"""

import os
import shutil
import subprocess
import tempfile
import unittest
from datetime import datetime, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo


class ReleaseVersionTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name)
        (self.repo / "scripts").mkdir()
        self.script = self.repo / "scripts/release-version.sh"
        shutil.copy2(Path(__file__).with_name("release-version.sh"), self.script)
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        self.env["TZ"] = "UTC"
        self.git("init", "--quiet")
        self.git(
            "-c", "user.name=Release Test", "-c", "user.email=release-test@example.invalid",
            "-c", "commit.gpgsign=false", "commit", "--quiet", "--allow-empty", "-m", "fixture",
        )
        self.today = datetime.now(ZoneInfo("Asia/Tokyo")).date().isoformat()

    def git(self, *args):
        return subprocess.run(
            ["git", *args], cwd=self.repo, env=self.env, check=True, capture_output=True, text=True,
        )

    def run_script(self, *args):
        # 作業ディレクトリに依存せず、スクリプトのリポジトリを参照する。
        return subprocess.run(
            ["sh", str(self.script), *args], cwd=self.repo.parent, env=self.env,
            capture_output=True, text=True,
        )

    def assert_next(self, expected):
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), expected)

    def test_first_release_uses_date_without_suffix(self):
        self.assert_next(self.today)

    def test_second_release_starts_at_one(self):
        self.git("tag", self.today)
        self.assert_next(f"{self.today}.1")

    def test_uses_numeric_maximum_and_does_not_reuse_gaps(self):
        for suffix in (None, 1, 2, 9, 10):
            self.git("tag", self.today if suffix is None else f"{self.today}.{suffix}")
        self.assert_next(f"{self.today}.11")

    def test_increments_even_when_base_tag_is_missing(self):
        self.git("tag", f"{self.today}.3")
        self.assert_next(f"{self.today}.4")

    def test_ignores_other_dates_legacy_and_malformed_tags(self):
        yesterday = (datetime.now(ZoneInfo("Asia/Tokyo")).date() - timedelta(days=1)).isoformat()
        for tag in (
            yesterday, f"{yesterday}.99", "v0.1.0", f"v{self.today}",
            f"{self.today}.0", f"{self.today}.01", f"{self.today}.9-rc.1",
            f"{self.today}.1.2", f"{self.today}-other",
        ):
            self.git("tag", tag)
        self.assert_next(self.today)

    def test_valid_existing_tag_does_not_depend_on_package_version(self):
        for tag in ("2026-10-13", "2026-10-13.1", "2026-10-13.10", "2024-02-29"):
            with self.subTest(tag=tag):
                result = self.run_script(tag)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), tag)

    def test_rejects_invalid_dates_and_noncanonical_suffixes(self):
        for tag in (
            "", "v2026-10-13", "0.1.0", "2026-1-3", "2026-02-29", "2026-13-01",
            "2026-04-31", "0000-01-01", "2026-10-13.0", "2026-10-13.01",
            "2026-10-13.-1", "2026-10-13.1.2", "2026-10-13.1\n",
        ):
            with self.subTest(tag=tag):
                result = self.run_script(tag)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")

    def test_rejects_extra_arguments(self):
        self.assertNotEqual(self.run_script("2026-10-13", "extra").returncode, 0)


if __name__ == "__main__":
    unittest.main()
