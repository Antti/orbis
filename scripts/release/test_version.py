"""Release decisions use the push boundary, not just the last commit."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import version


SCRIPT = Path(version.__file__).resolve()


def write_manifest(root: Path, release_version: str) -> None:
    path = root / version.MANIFEST
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f'[package]\nname = "orbis-cli"\nversion = "{release_version}"\n')


class GitFixture:
    def __init__(self, root: Path):
        self.root = root
        self.env = {
            **os.environ,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
        }
        self.git("init", "--quiet")
        self.git("config", "user.name", "Release tests")
        self.git("config", "user.email", "release-tests@example.invalid")

    def git(self, *args: str) -> str:
        return subprocess.check_output(
            ["git", *args], cwd=self.root, env=self.env, text=True, stderr=subprocess.PIPE
        ).strip()

    def commit(self, release_version: str, message: str) -> str:
        write_manifest(self.root, release_version)
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "--quiet", "-m", message)
        return self.git("rev-parse", "HEAD")


class VersionTests(unittest.TestCase):
    def test_stable_versions_require_canonical_three_part_numbers(self):
        for text, expected in [
            ("0.0.0", (0, 0, 0)),
            ("1.2.3", (1, 2, 3)),
            ("12.34.567", (12, 34, 567)),
        ]:
            with self.subTest(version=text):
                self.assertEqual(version.parse_version(text), expected)
        for text in [
            "", "1", "1.2", "1.2.3.4", "v1.2.3", "01.2.3", "1.02.3", "1.2.03",
            "-1.2.3", "1.2.3-rc.1", "1.2.3+build", "1.2.3\n", " 1.2.3", "１.2.3",
        ]:
            with self.subTest(version=text):
                with self.assertRaises(ValueError):
                    version.parse_version(text)

    def test_manifest_requires_the_cli_package_and_a_valid_version(self):
        self.assertEqual(
            version.manifest_version('[package]\nname="orbis-cli"\nversion="2.3.4"'),
            "2.3.4",
        )
        for source in [
            '[package]\nname="orbis-pfs"\nversion="2.3.4"',
            '[package]\nname="orbis-pkg-util"\nversion="2.3.4"',
            '[package]\nname="orbis-cli"\nversion="2.3.4-rc.1"',
            '[package]\nname="orbis-cli"\nversion=23',
            '[package]\nname="orbis-cli"\nversion.workspace=true',
            '[package]\nname="orbis-cli"',
            "[workspace]",
            "[broken",
        ]:
            with self.subTest(source=source):
                with self.assertRaises((ValueError, KeyError, TypeError)):
                    version.manifest_version(source)

    def test_numeric_version_order_rejects_downgrades(self):
        for previous, current in [("1.9.9", "1.10.0"), ("1.9.9", "2.0.0"), ("0.0.0", "0.0.1")]:
            self.assertTrue(version.is_increase(previous, current))
        self.assertFalse(version.is_increase("1.2.3", "1.2.3"))
        for previous, current in [("2.0.0", "1.99.99"), ("1.10.0", "1.9.99"), ("1.2.3", "1.2.2")]:
            with self.subTest(previous=previous, current=current):
                with self.assertRaisesRegex(ValueError, "decreased"):
                    version.is_increase(previous, current)

    def test_event_gates_never_publish_previews_or_new_branches(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            write_manifest(root, "1.2.3")
            for event, ref, before, build in [
                ("pull_request", "refs/pull/7/merge", "", True),
                ("workflow_dispatch", "refs/heads/master", "invalid", True),
                ("push", "refs/heads/feature", "invalid", False),
                ("push", "refs/tags/v1.2.3", "invalid", False),
                ("push", "refs/heads/master", "", False),
                ("push", "refs/heads/master", "0" * 40, False),
                ("schedule", "refs/heads/master", "a" * 40, False),
            ]:
                with self.subTest(event=event, ref=ref, before=before):
                    with patch.object(version.subprocess, "check_output") as git:
                        self.assertEqual(
                            version.plan(event, ref, before, root),
                            {"version": "1.2.3", "tag": "v1.2.3", "build": str(build).lower(), "publish": "false"},
                        )
                        git.assert_not_called()

    def test_multicommit_push_detects_an_earlier_version_bump(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture = GitFixture(root)
            before = fixture.commit("1.2.3", "Previous release")
            bumped = fixture.commit("1.3.0", "Bump version")
            (root / "README.md").write_text("Documentation added after the bump.\n")
            fixture.commit("1.3.0", "Document new release")
            plan = version.plan("push", "refs/heads/master", before, root)
            self.assertEqual(plan, {"version": "1.3.0", "tag": "v1.3.0", "build": "true", "publish": "true"})
            self.assertEqual(
                version.plan("pull_request", "refs/pull/7/merge", before, root),
                {"version": "1.3.0", "tag": "v1.3.0", "build": "true", "publish": "false"},
            )
            # Comparing only HEAD^ would incorrectly suppress this release.
            self.assertEqual(version.plan("push", "refs/heads/master", bumped, root)["publish"], "false")

    def test_real_git_unchanged_and_downgraded_versions_are_not_released(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture = GitFixture(root)
            before = fixture.commit("1.10.0", "Previous release")
            (root / "notes.txt").write_text("No version change.\n")
            fixture.commit("1.10.0", "Documentation only")
            self.assertEqual(
                version.plan("push", "refs/heads/master", before, root),
                {"version": "1.10.0", "tag": "v1.10.0", "build": "false", "publish": "false"},
            )
            fixture.commit("1.9.0", "Accidental downgrade")
            for event, ref in [("push", "refs/heads/master"), ("pull_request", "refs/pull/7/merge")]:
                with self.assertRaisesRegex(ValueError, "decreased"):
                    version.plan(event, ref, before, root)

    def test_package_rename_preserves_version_comparisons(self):
        for current in ["1.2.3", "1.3.0", "1.2.2"]:
            with self.subTest(current=current), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                fixture = GitFixture(root)
                legacy = root / version.LEGACY_MANIFEST
                legacy.parent.mkdir()
                legacy.write_text('[package]\nname="orbis-pkg-util"\nversion="1.2.3"\n')
                fixture.git("add", ".")
                fixture.git("-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "Before rename")
                before = fixture.git("rev-parse", "HEAD")
                legacy.unlink()
                fixture.commit(current, "Rename the package")
                for event, ref in [("push", "refs/heads/master"), ("pull_request", "refs/pull/7/merge")]:
                    if current == "1.2.2":
                        with self.assertRaisesRegex(ValueError, "decreased"):
                            version.plan(event, ref, before, root)
                    else:
                        plan = version.plan(event, ref, before, root)
                        self.assertEqual(plan["publish"], str(event == "push" and current == "1.3.0").lower())
                        self.assertEqual(plan["build"], str(event == "pull_request" or current == "1.3.0").lower())

    def test_historical_manifest_fallback_does_not_hide_invalid_state(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture = GitFixture(root)
            legacy = root / version.LEGACY_MANIFEST
            legacy.parent.mkdir()
            legacy.write_text('[package]\nname="orbis-pkg-util"\nversion="1.2.3"\n')
            before = fixture.commit("1.3.0", "Both paths exist")
            self.assertEqual(version.previous_version(root, before), "1.3.0")
            (root / version.MANIFEST).write_text('[package]\nname="other"\nversion="1.3.0"\n')
            fixture.git("add", ".")
            fixture.git("-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "Invalid new manifest")
            before = fixture.git("rev-parse", "HEAD")
            with self.assertRaisesRegex(ValueError, "orbis-cli"):
                version.previous_version(root, before)
            (root / version.MANIFEST).unlink()
            legacy.unlink()
            fixture.git("add", ".")
            fixture.git("-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "Missing manifests")
            before = fixture.git("rev-parse", "HEAD")
            with self.assertRaisesRegex(ValueError, "no CLI manifest"):
                version.previous_version(root, before)

    def test_bad_or_unavailable_previous_commits_fail_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture = GitFixture(root)
            fixture.commit("1.2.3", "Initial release")
            for before in ["HEAD", "--help", "A" * 40, "a" * 39, "a" * 41, "a" * 40 + "\n"]:
                with self.subTest(before=before):
                    with patch.object(version.subprocess, "check_output") as git:
                        with self.assertRaisesRegex(ValueError, "SHA"):
                            version.plan("push", "refs/heads/master", before, root)
                        git.assert_not_called()
            with self.assertRaises(subprocess.CalledProcessError):
                version.plan("push", "refs/heads/master", "f" * 40, root)

    def test_cli_prints_and_appends_github_outputs(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            write_manifest(root, "1.2.3")
            output_path = root / "github-output"
            output_path.write_text("previous=value\n")
            env = {**os.environ, "GITHUB_OUTPUT": str(output_path)}
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--event", "workflow_dispatch", "--ref", "refs/heads/master"],
                cwd=root, env=env, text=True, capture_output=True, check=True,
            )
            expected = "version=1.2.3\ntag=v1.2.3\nbuild=true\npublish=false\n"
            self.assertEqual(result.stdout, expected)
            self.assertEqual(output_path.read_text(), "previous=value\n" + expected)
            env.pop("GITHUB_OUTPUT")
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--event", "workflow_dispatch", "--ref", "refs/heads/master"],
                cwd=root, env=env, text=True, capture_output=True, check=True,
            )
            self.assertEqual(result.stdout, expected)


if __name__ == "__main__":
    unittest.main()
