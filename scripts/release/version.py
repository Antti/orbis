#!/usr/bin/env python3
"""Decide whether a push increases the CLI's release version (Python 3.11+)."""

import argparse
import os
from pathlib import Path
import re
import subprocess
import tomllib

MANIFEST = "orbis-cli/Cargo.toml"
LEGACY_MANIFEST = "orbis-pkg-util/Cargo.toml"
STABLE_VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")


def parse_version(version: str) -> tuple[int, int, int]:
    match = STABLE_VERSION.fullmatch(version)
    if match is None:
        raise ValueError(f"Expected a stable major.minor.patch version, got {version!r}")
    return tuple(int(part) for part in match.groups())


def manifest_version(contents: str, *, package_name: str = "orbis-cli") -> str:
    package = tomllib.loads(contents)["package"]
    if package["name"] != package_name:
        raise ValueError(f"Release version must come from {package_name}")
    version = package["version"]
    parse_version(version)
    return version


def is_increase(previous: str, current: str) -> bool:
    old, new = parse_version(previous), parse_version(current)
    if new < old:
        raise ValueError(f"Release version decreased from {previous} to {current}")
    return new > old


def previous_version(root: Path, commit: str) -> str:
    # The push/PR boundary can precede the package rename. Prefer the new path
    # when present, and never conceal a malformed new manifest with a fallback.
    paths = subprocess.check_output(
        ["git", "ls-tree", "--name-only", commit, "--", MANIFEST, LEGACY_MANIFEST],
        cwd=root, text=True,
    ).splitlines()
    if MANIFEST in paths:
        manifest, package_name = MANIFEST, "orbis-cli"
    elif LEGACY_MANIFEST in paths:
        manifest, package_name = LEGACY_MANIFEST, "orbis-pkg-util"
    else:
        raise ValueError(f"Previous commit {commit} has no CLI manifest")
    contents = subprocess.check_output(
        ["git", "show", f"{commit}:{manifest}"], cwd=root, text=True
    )
    return manifest_version(contents, package_name=package_name)


def plan(event: str, ref: str, before: str, root: Path) -> dict[str, str]:
    current = manifest_version((root / MANIFEST).read_text())
    publish = False
    compare = event == "pull_request" or (event == "push" and ref == "refs/heads/master")
    if compare:
        # A newly created branch has no previous version to compare.
        if before and before != "0" * 40:
            if not re.fullmatch(r"[0-9a-f]{40}", before):
                raise ValueError("Invalid previous commit SHA")
            increased = is_increase(previous_version(root, before), current)
            publish = event == "push" and increased
    return {
        "version": current,
        "tag": f"v{current}",
        "build": str(event in ("pull_request", "workflow_dispatch") or publish).lower(),
        "publish": str(publish).lower(),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--event", required=True)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--before", default="")
    args = parser.parse_args()
    values = plan(args.event, args.ref, args.before, Path.cwd())
    output = "".join(f"{key}={value}\n" for key, value in values.items())
    print(output, end="")
    if path := os.environ.get("GITHUB_OUTPUT"):
        with open(path, "a") as stream:
            stream.write(output)


if __name__ == "__main__":
    main()
