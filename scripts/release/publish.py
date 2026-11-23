#!/usr/bin/env python3
"""Publish tested assets through a draft release; never replace a public release."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

from version import parse_version


def gh(*args: str) -> str:
    return subprocess.check_output(["gh", *args], text=True)


def expected_assets(version: str) -> list[str]:
    parse_version(version)
    return [
        f"orbis-cli-{version}-linux-x86_64.AppImage",
        f"orbis-cli-{version}-linux-x86_64.flatpak",
        f"orbis-cli-{version}-linux-aarch64.AppImage",
        f"orbis-cli-{version}-linux-aarch64.flatpak",
        f"orbis-cli-{version}-x86_64-pc-windows-msvc.zip",
        f"orbis-cli-{version}-x86_64-apple-darwin.tar.gz",
        f"orbis-cli-{version}-aarch64-apple-darwin.tar.gz",
        f"orbis-desktop-{version}-x86_64-unknown-linux-gnu.tar.gz",
        f"orbis-desktop-{version}-aarch64-unknown-linux-gnu.tar.gz",
        f"orbis-desktop-{version}-x86_64-pc-windows-msvc.zip",
        f"orbis-desktop-{version}-x86_64-apple-darwin.tar.gz",
        f"orbis-desktop-{version}-aarch64-apple-darwin.tar.gz",
    ]


def write_checksums(directory: Path, version: str) -> dict[str, str]:
    expected = expected_assets(version)
    actual = {path.name for path in directory.iterdir()}
    if actual - {"SHA256SUMS"} != set(expected):
        raise ValueError(f"Expected exactly these release assets: {expected}; found {sorted(actual)}")
    digests = {}
    for name in sorted(expected):
        path = directory / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"Invalid release asset: {path}")
        with path.open("rb") as stream:
            digests[name] = hashlib.file_digest(stream, "sha256").hexdigest()
    checksum_file = directory / "SHA256SUMS"
    if checksum_file.is_symlink() or (checksum_file.exists() and not checksum_file.is_file()):
        raise ValueError(f"Invalid checksum file: {checksum_file}")
    manifest = "".join(f"{digest}  {name}\n" for name, digest in digests.items()).encode()
    checksum_file.write_bytes(manifest)
    digests["SHA256SUMS"] = hashlib.sha256(manifest).hexdigest()
    return digests


def publish(
    repo: str, sha: str, version: str, directory: Path, *, checksums_sha256: str | None = None
) -> None:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise ValueError("Invalid repository name")
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Invalid release commit SHA")
    digests = write_checksums(directory, version)
    if checksums_sha256 is not None and digests["SHA256SUMS"] != checksums_sha256:
        raise ValueError("Release assets changed since checksum preparation")
    tag = f"v{version}"
    # Fail on API/authentication errors rather than treating them as missing tags.
    refs = json.loads(gh("api", f"repos/{repo}/git/matching-refs/tags/{tag}"))
    ref = next((item for item in refs if item["ref"] == f"refs/tags/{tag}"), None)
    if ref:
        # Resolve annotated tags too, without moving any existing reference.
        target = json.loads(gh("api", f"repos/{repo}/commits/{tag}"))["sha"]
        if target != sha:
            raise ValueError(f"{tag} already points to {target}, refusing to release {sha}")
    else:
        gh(
            "api", "--method", "POST", f"repos/{repo}/git/refs",
            "-f", f"ref=refs/tags/{tag}", "-f", f"sha={sha}",
        )

    releases = json.loads(
        gh("api", "--paginate", "--slurp", f"repos/{repo}/releases?per_page=100")
    )
    release = next(
        (item for page in releases for item in page if item["tag_name"] == tag), None
    )
    if release and not release["draft"]:
        print(f"{tag} is already published at the correct commit; leaving its assets unchanged.")
        return
    if release:
        unexpected = {asset["name"] for asset in release["assets"]} - set(
            expected_assets(version) + ["SHA256SUMS"]
        )
        if unexpected:
            raise ValueError(f"Draft release contains unexpected assets: {sorted(unexpected)}")
    if release is None:
        notes = (
            f"Built from `{sha}`. Includes Linux x86_64/ARM64 AppImage/Flatpak bundles, "
            "Windows x86_64 ZIP, and macOS Intel/Apple Silicon archives. Choose orbis-cli-* for the orbis command or orbis-desktop-* for the graphical app. "
            f"[Verify and install the downloads](https://github.com/{repo}/blob/{tag}/RELEASE.md#installing-packages). "
            "Mac signing status is in BUILD-INFO.txt."
        )
        release = json.loads(
            gh(
                "api", "--method", "POST", f"repos/{repo}/releases",
                "-f", f"tag_name={tag}", "-f", f"target_commitish={sha}",
                "-f", f"name=Orbis {version}",
                "-F", "draft=true", "-F", "generate_release_notes=true",
                "-f", f"body={notes}",
            )
        )
    # Only draft assets may be replaced when retrying an interrupted upload.
    assets = [str(directory / name) for name in expected_assets(version) + ["SHA256SUMS"]]
    gh("release", "upload", tag, "--repo", repo, "--clobber", *assets)
    # The tag endpoint only finds published releases; drafts need their numeric ID.
    uploaded = json.loads(gh("api", f"repos/{repo}/releases/{release['id']}"))
    uploaded_files = {
        asset["name"]: (asset["size"], asset.get("digest")) for asset in uploaded["assets"]
    }
    expected_files = {
        name: ((directory / name).stat().st_size, f"sha256:{digest}") for name, digest in digests.items()
    }
    if not uploaded["draft"] or uploaded_files != expected_files:
        raise ValueError("Draft release state or uploaded assets changed; refusing publication")
    # Semantic-version ordering avoids making an older, slower build Latest.
    gh(
        "api", "--method", "PATCH", f"repos/{repo}/releases/{uploaded['id']}",
        "-F", "draft=false", "-f", "make_latest=legacy",
    )
    print(f"Published {tag} from {sha}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("directory", type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--checksums-only", action="store_true", help="Prepare assets and print the manifest digest")
    mode.add_argument("--checksums-sha256", help="Require the prepared manifest digest before uploading")
    args = parser.parse_args()
    if args.checksums_only:
        digests = write_checksums(args.directory, args.version)
        print(f"checksums_sha256={digests['SHA256SUMS']}")
        return
    publish(
        os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_SHA"], args.version, args.directory,
        checksums_sha256=args.checksums_sha256,
    )


if __name__ == "__main__":
    main()
