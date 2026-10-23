#!/usr/bin/env python3
"""Package and smoke-test a Windows x86-64 CLI archive."""
import argparse
from pathlib import Path
import subprocess
import tempfile
import zipfile
from version import parse_version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    parse_version(args.version)
    args.output.mkdir(parents=True, exist_ok=True)
    name = f"orbis-cli-{args.version}-x86_64-pc-windows-msvc"
    subprocess.run([str(args.binary.resolve()), "--help"], check=True)
    archive = args.output / f"{name}.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as dest:
        dest.write(args.binary, f"{name}/orbis.exe")
        # A second executable also works after ordinary Windows ZIP extraction;
        # a symlink would require special permissions on some installations.
        dest.write(args.binary, f"{name}/orbis-pkg-util.exe")
        for license_file in ("LICENSE-MIT", "LICENSE-APACHE"):
            dest.write(license_file, f"{name}/{license_file}")
    with tempfile.TemporaryDirectory() as directory:
        with zipfile.ZipFile(archive) as source:
            source.extractall(directory)
        subprocess.run([str(Path(directory) / name / "orbis.exe"), "--help"], check=True)
        subprocess.run([str(Path(directory) / name / "orbis-pkg-util.exe"), "--help"], check=True)


if __name__ == "__main__":
    main()
