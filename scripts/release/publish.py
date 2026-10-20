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
        f"orbis-pkg-util-{version}-linux-x86_64.AppImage",
        f"orbis-pkg-util-{version}-linux-x86_64.flatpak",
        f"orbis-pkg-util-{version}-x86_64-apple-darwin.tar.gz",
        f"orbis-pkg-util-{version}-aarch64-apple-darwin.tar.gz",
    ]


def write_checksums(directory: Path, version: str) -> None:
    expected = expected_assets(version)
    actual = {path.name for path in directory.iterdir()}
    if actual - {"SHA256SUMS"} != set(expected):
        raise ValueError(f"Expected exactly these release assets: {expected}; found {sorted(actual)}")
    lines = []
    for name in sorted(expected):
        path = directory / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"Invalid release asset: {path}")
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        lines.append(f"{digest}  {name}\n")
    checksum_file = directory / "SHA256SUMS"
    if checksum_file.is_symlink() or (checksum_file.exists() and not checksum_file.is_file()):
        raise ValueError(f"Invalid checksum file: {checksum_file}")
    checksum_file.write_text("".join(lines))


def publish(repo: str, sha: str, version: str, directory: Path) -> None:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise ValueError("Invalid repository name")
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Invalid release commit SHA")
    write_checksums(directory, version)
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
            f"Built from `{sha}`. Includes Linux x86_64 AppImage/Flatpak bundles "
            "and unsigned macOS Intel/Apple Silicon CLI archives. "
            "Verify downloads with SHA256SUMS. See RELEASE.md for installation."
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
    sizes = {asset["name"]: asset["size"] for asset in uploaded["assets"]}
    expected_sizes = {Path(asset).name: Path(asset).stat().st_size for asset in assets}
    if not uploaded["draft"] or sizes != expected_sizes:
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
    args = parser.parse_args()
    publish(os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_SHA"], args.version, args.directory)


if __name__ == "__main__":
    main()
