"""Exercise publication locally; every GitHub call is mocked."""

from contextlib import redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import publish


REPO = "example/orbis"
SHA = "a" * 40
VERSION = "1.2.3"
TAG = f"v{VERSION}"
ASSETS = [
    f"orbis-pkg-util-{VERSION}-linux-x86_64.AppImage",
    f"orbis-pkg-util-{VERSION}-linux-x86_64.flatpak",
    f"orbis-pkg-util-{VERSION}-x86_64-apple-darwin.tar.gz",
    f"orbis-pkg-util-{VERSION}-aarch64-apple-darwin.tar.gz",
]


def make_assets(directory: Path) -> None:
    for index, name in enumerate(ASSETS):
        (directory / name).write_bytes(f"artifact {index}\n".encode())


class FakeGitHub:
    """An intentionally small state machine that rejects unexpected GH calls."""

    def __init__(
        self, tag_sha=None, draft=None, fail=None, assets=(), verification=None, release_id=123
    ):
        self.tag_sha = tag_sha
        self.draft = draft
        self.fail = fail
        self.calls = []
        self.uploads = []
        self.assets = list(assets)
        self.verification = verification
        self.release_id = release_id

    def __call__(self, *args):
        self.calls.append(args)
        if args == ("api", f"repos/{REPO}/git/matching-refs/tags/{TAG}"):
            self.maybe_fail("refs")
            refs = [{"ref": f"refs/tags/{TAG}0"}]  # Prefix matches must be ignored.
            if self.tag_sha is not None:
                refs.append({"ref": f"refs/tags/{TAG}", "object": {"type": "tag"}})
            return json.dumps(refs)
        if args == ("api", f"repos/{REPO}/commits/{TAG}"):
            self.maybe_fail("resolve")
            return json.dumps({"sha": self.tag_sha})
        if args == ("api", "--method", "POST", f"repos/{REPO}/git/refs", "-f", f"ref=refs/tags/{TAG}", "-f", f"sha={SHA}"):
            self.maybe_fail("tag")
            if self.tag_sha is not None:
                raise AssertionError("Existing tags must never be overwritten")
            self.tag_sha = SHA
            return "{}"
        if args == ("api", "--paginate", "--slurp", f"repos/{REPO}/releases?per_page=100"):
            self.maybe_fail("releases")
            matching = [] if self.draft is None else [
                {
                    "id": self.release_id, "tag_name": TAG, "draft": self.draft,
                    "assets": [{"name": name} for name in self.assets],
                }
            ]
            return json.dumps([[{"tag_name": "v0.0.1", "draft": False}], matching])
        if args[:4] == ("api", "--method", "POST", f"repos/{REPO}/releases"):
            self.maybe_fail("create")
            if self.draft is not None or self.tag_sha != SHA:
                raise AssertionError("Draft requires an existing matching tag and no release")
            if len(args[4:]) % 2:
                raise AssertionError("Creation fields must be flag/value pairs")
            fields = {}
            for flag, value in zip(args[4::2], args[5::2]):
                name, separator, data = value.partition("=")
                if not separator or name in fields:
                    raise AssertionError(f"Invalid or duplicate creation field: {value}")
                fields[name] = (flag, data)
            expected_fields = {
                "tag_name": ("-f", TAG),
                "target_commitish": ("-f", SHA),
                "name": ("-f", f"Orbis {VERSION}"),
                "draft": ("-F", "true"),
                "generate_release_notes": ("-F", "true"),
                "body": (
                    "-f",
                    f"Built from `{SHA}`. Includes Linux x86_64 AppImage/Flatpak bundles "
                    "and unsigned macOS Intel/Apple Silicon CLI archives. "
                    "Verify downloads with SHA256SUMS. See RELEASE.md for installation.",
                ),
            }
            if fields != expected_fields:
                raise AssertionError(f"Unexpected release creation fields: {fields}")
            self.draft = True
            return self.release_response()
        if args[:3] == ("release", "upload", TAG):
            if self.fail == "upload":
                # A failed multi-file upload may already have installed a subset.
                self.assets = ASSETS[:2]
            self.maybe_fail("upload")
            if self.draft is not True:
                raise AssertionError("Cannot replace assets on a public release")
            if args[3:6] != ("--repo", REPO, "--clobber"):
                raise AssertionError("Draft retries must use the explicit repository")
            paths = [Path(value) for value in args[6:]]
            if {path.name for path in paths} != set(ASSETS + ["SHA256SUMS"]):
                raise AssertionError("Upload must contain exactly the required assets")
            self.uploads.append(paths)
            self.assets = [path.name for path in paths]
            return ""
        if args == ("api", f"repos/{REPO}/releases/tags/{TAG}"):
            # GitHub's by-tag endpoint cannot retrieve a draft, even when the
            # token can list it, upload to it, and retrieve it by release ID.
            if self.draft is not False:
                raise subprocess.CalledProcessError(
                    1, ["gh", *args], stderr="gh: Not Found (HTTP 404)"
                )
            return self.release_response()
        if args == ("api", f"repos/{REPO}/releases/{self.release_id}"):
            self.maybe_fail("verify")
            if self.draft is None:
                raise subprocess.CalledProcessError(
                    1, ["gh", *args], stderr="gh: Not Found (HTTP 404)"
                )
            return self.release_response()
        if args == (
            "api", "--method", "PATCH", f"repos/{REPO}/releases/{self.release_id}",
            "-F", "draft=false", "-f", "make_latest=legacy",
        ):
            self.maybe_fail("edit")
            if self.draft is not True or not self.uploads:
                raise AssertionError("Cannot publish before all files are uploaded")
            self.draft = False
            return ""
        raise AssertionError(f"Unexpected GitHub command: {args}")

    def release_response(self):
        sizes = {path.name: path.stat().st_size for path in self.uploads[-1]} if self.uploads else {}
        assets = [{"name": name, "size": sizes.get(name, 0)} for name in self.assets]
        if self.uploads:
            if self.verification == "extra":
                assets.append({"name": "extra.zip", "size": 3})
            elif self.verification == "missing":
                assets.pop()
            elif self.verification == "size":
                assets[0]["size"] += 1
        return json.dumps({
            "id": self.release_id,
            "draft": False if self.verification == "public" else self.draft,
            "assets": assets,
        })

    def maybe_fail(self, stage):
        if self.fail == stage:
            self.fail = None
            raise subprocess.CalledProcessError(1, ["gh", stage])


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        make_assets(self.directory)

    def run_publish(self, remote):
        with patch.object(publish, "gh", side_effect=remote), redirect_stdout(io.StringIO()):
            publish.publish(REPO, SHA, VERSION, self.directory)

    def test_checksums_match_exact_asset_names_contents_and_sorted_order(self):
        self.assertEqual(publish.expected_assets(VERSION), ASSETS)
        (self.directory / "SHA256SUMS").write_text("stale\n")
        publish.write_checksums(self.directory, VERSION)
        expected = "".join(
            f"{hashlib.sha256((self.directory / name).read_bytes()).hexdigest()}  {name}\n"
            for name in sorted(ASSETS)
        )
        self.assertEqual((self.directory / "SHA256SUMS").read_text(), expected)
        publish.write_checksums(self.directory, VERSION)
        self.assertEqual((self.directory / "SHA256SUMS").read_text(), expected)

    def test_invalid_asset_sets_fail_before_github_access(self):
        for invalid in ["missing", "extra", "empty", "directory", "symlink"]:
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory() as temp:
                directory = Path(temp)
                make_assets(directory)
                path = directory / ASSETS[0]
                if invalid == "extra":
                    (directory / "unexpected.zip").write_bytes(b"unexpected")
                elif invalid == "empty":
                    path.write_bytes(b"")
                else:
                    path.unlink()
                    if invalid == "directory":
                        path.mkdir()
                    elif invalid == "symlink":
                        path.symlink_to(directory / ASSETS[1])
                with patch.object(publish, "gh") as gh:
                    with self.assertRaises(ValueError):
                        publish.publish(REPO, SHA, VERSION, directory)
                    gh.assert_not_called()
                self.assertFalse((directory / "SHA256SUMS").exists())

    def test_checksum_symlinks_cannot_overwrite_external_files(self):
        with tempfile.TemporaryDirectory() as temp:
            external = Path(temp) / "keep.txt"
            external.write_text("keep this content")
            (self.directory / "SHA256SUMS").symlink_to(external)
            with self.assertRaises(ValueError):
                publish.write_checksums(self.directory, VERSION)
            self.assertEqual(external.read_text(), "keep this content")
        checksum = self.directory / "SHA256SUMS"
        checksum.unlink()
        checksum.mkdir()
        with self.assertRaises(ValueError):
            publish.write_checksums(self.directory, VERSION)

    def test_invalid_repository_sha_or_version_never_calls_github(self):
        for repo, sha, release_version in [
            ("../owner/repo", SHA, VERSION),
            ("owner/repo/extra", SHA, VERSION),
            ("owner/repo\n", SHA, VERSION),
            (REPO, "HEAD", VERSION),
            (REPO, "a" * 39, VERSION),
            (REPO, "A" * 40, VERSION),
            (REPO, SHA, "1.2.3-rc.1"),
            (REPO, SHA, "../1.2.3"),
        ]:
            with self.subTest(repo=repo, sha=sha, version=release_version):
                with patch.object(publish, "gh") as gh:
                    with self.assertRaises(ValueError):
                        publish.publish(repo, sha, release_version, self.directory)
                    gh.assert_not_called()

    def test_new_release_is_published_only_after_asset_verification(self):
        remote = FakeGitHub()
        self.run_publish(remote)
        self.assertEqual(remote.tag_sha, SHA)
        self.assertFalse(remote.draft)
        self.assertEqual(
            [call[:3] for call in remote.calls],
            [
                ("api", f"repos/{REPO}/git/matching-refs/tags/{TAG}"),
                ("api", "--method", "POST"),
                ("api", "--paginate", "--slurp"),
                ("api", "--method", "POST"),
                ("release", "upload", TAG),
                ("api", f"repos/{REPO}/releases/123"),
                ("api", "--method", "PATCH"),
            ],
        )
        self.assertEqual(len(remote.uploads), 1)
        self.assertTrue(all(path.is_file() for path in remote.uploads[0]))
        for verification in ["extra", "missing", "size", "public"]:
            with self.subTest(verification=verification):
                remote = FakeGitHub(verification=verification)
                with self.assertRaisesRegex(ValueError, "refusing publication"):
                    self.run_publish(remote)
                self.assertFalse(any(call[:3] == ("api", "--method", "PATCH") for call in remote.calls))
                self.assertTrue(remote.draft)

    def test_annotated_matching_tag_is_resolved_and_never_recreated(self):
        remote = FakeGitHub(tag_sha=SHA)
        self.run_publish(remote)
        self.assertIn(("api", f"repos/{REPO}/commits/{TAG}"), remote.calls)
        self.assertFalse(any(
            call[:4] == ("api", "--method", "POST", f"repos/{REPO}/git/refs")
            for call in remote.calls
        ))
        self.assertFalse(remote.draft)

    def test_new_and_resumed_drafts_are_verified_by_id_when_tag_lookup_returns_404(self):
        for resumed, release_id in [(False, 418330940), (True, 418330941)]:
            with self.subTest(resumed=resumed, release_id=release_id):
                remote = FakeGitHub(
                    tag_sha=SHA if resumed else None,
                    draft=True if resumed else None,
                    release_id=release_id,
                )
                if resumed:
                    with self.assertRaises(subprocess.CalledProcessError) as error:
                        remote("api", f"repos/{REPO}/releases/tags/{TAG}")
                    self.assertIn("HTTP 404", error.exception.stderr)
                    existing = json.loads(remote("api", f"repos/{REPO}/releases/{release_id}"))
                    self.assertEqual(existing["id"], release_id)
                    self.assertTrue(existing["draft"])
                    remote.calls.clear()
                self.run_publish(remote)
                self.assertNotIn(("api", f"repos/{REPO}/releases/tags/{TAG}"), remote.calls)
                self.assertIn(("api", f"repos/{REPO}/releases/{release_id}"), remote.calls)
                self.assertEqual(
                    sum(
                        call[:4] == ("api", "--method", "POST", f"repos/{REPO}/releases")
                        for call in remote.calls
                    ),
                    0 if resumed else 1,
                )
                self.assertFalse(remote.draft)

    def test_tag_pointing_to_another_commit_fails_without_mutations(self):
        remote = FakeGitHub(tag_sha="b" * 40)
        with self.assertRaisesRegex(ValueError, "already points"):
            self.run_publish(remote)
        self.assertEqual(len(remote.calls), 2)
        self.assertEqual(remote.tag_sha, "b" * 40)
        self.assertIsNone(remote.draft)
        self.assertFalse(remote.uploads)

    def test_published_release_is_immutable_on_retry(self):
        remote = FakeGitHub(tag_sha=SHA, draft=False)
        self.run_publish(remote)
        self.assertFalse(any(call[0] == "release" for call in remote.calls))
        self.assertFalse(remote.uploads)
        self.assertFalse(remote.draft)

    def test_existing_drafts_resume_only_with_expected_assets(self):
        for assets in [[], ASSETS[:2], ASSETS + ["SHA256SUMS"]]:
            with self.subTest(assets=assets):
                remote = FakeGitHub(tag_sha=SHA, draft=True, assets=assets)
                self.run_publish(remote)
                self.assertFalse(any(
                    call[:4] == ("api", "--method", "POST", f"repos/{REPO}/releases")
                    for call in remote.calls
                ))
                self.assertFalse(any(call[:3] == ("api", "--method", "POST") for call in remote.calls))
                self.assertEqual(len(remote.uploads), 1)
                self.assertFalse(remote.draft)
        remote = FakeGitHub(tag_sha=SHA, draft=True, assets=["unexpected.zip"])
        with self.assertRaisesRegex(ValueError, "unexpected assets"):
            self.run_publish(remote)
        self.assertFalse(any(call[0] == "release" for call in remote.calls))
        self.assertTrue(remote.draft)

    def test_interrupted_create_upload_or_publish_can_be_retried(self):
        for failure in ["create", "upload", "edit"]:
            with self.subTest(failure=failure):
                remote = FakeGitHub(fail=failure)
                with self.assertRaises(subprocess.CalledProcessError):
                    self.run_publish(remote)
                self.assertEqual(remote.tag_sha, SHA)
                self.assertIsNot(remote.draft, False)
                if failure == "upload":
                    self.assertFalse(any(call[:3] == ("api", "--method", "PATCH") for call in remote.calls))
                self.run_publish(remote)
                self.assertFalse(remote.draft)
                self.assertEqual(
                    sum(
                        call[:4] == ("api", "--method", "POST", f"repos/{REPO}/git/refs")
                        for call in remote.calls
                    ),
                    1,
                )
                self.assertEqual(
                    sum(
                        call[:4] == ("api", "--method", "POST", f"repos/{REPO}/releases")
                        for call in remote.calls
                    ),
                    2 if failure == "create" else 1,
                )
                # Once public, a further rerun must not replace any assets.
                uploads = len(remote.uploads)
                self.run_publish(remote)
                self.assertEqual(len(remote.uploads), uploads)

    def test_api_and_auth_failures_are_not_treated_as_missing_tags(self):
        for failure in ["refs", "resolve", "releases", "tag", "verify"]:
            with self.subTest(failure=failure):
                remote = FakeGitHub(tag_sha=SHA if failure == "resolve" else None, fail=failure)
                with self.assertRaises(subprocess.CalledProcessError):
                    self.run_publish(remote)
                self.assertFalse(any(call[:3] == ("api", "--method", "PATCH") for call in remote.calls))
                if failure != "verify":
                    self.assertFalse(any(call[0] == "release" for call in remote.calls))
                    self.assertFalse(remote.uploads)
                if failure in ["refs", "resolve"]:
                    self.assertFalse(any(call[:3] == ("api", "--method", "POST") for call in remote.calls))

    def test_cli_and_gh_adapter_pass_arguments_without_shell_interpolation(self):
        with patch.object(publish.subprocess, "check_output", return_value="response") as command:
            self.assertEqual(publish.gh("api", "repos/example/orbis"), "response")
            command.assert_called_once_with(["gh", "api", "repos/example/orbis"], text=True)
        with (
            patch.object(publish, "publish") as release,
            patch("sys.argv", ["publish.py", VERSION, str(self.directory)]),
            patch.dict(os.environ, {"GITHUB_REPOSITORY": REPO, "GITHUB_SHA": SHA}),
        ):
            publish.main()
            release.assert_called_once_with(REPO, SHA, VERSION, self.directory)


if __name__ == "__main__":
    unittest.main()
