#!/usr/bin/env python3
"""Validate a checksum-pinned public homebrew PKG; no commercial packages in CI."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import urllib.request

URL = "https://github.com/bucanero/ps4load/releases/download/v0.5.0/IV0000-LOAD00044_00-SDL2GLES20000000.pkg"
SHA256 = "00efee11c26404d2b6f350bcfd368c448fd0aa4863b74cd81216e4345092357e"


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            result.update(chunk)
    return result.hexdigest()


def run(binary, *args, expected=0):
    result = subprocess.run([str(binary), *map(str, args)], capture_output=True, text=True)
    if result.returncode != expected:
        raise RuntimeError(f"{args[0]} exited {result.returncode}, expected {expected}: {result.stderr}")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--package", type=Path, help="Use a local copy instead of downloading")
    parser.add_argument("--output-parent", type=Path, help="Optional mounted test volume")
    args = parser.parse_args()
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="orbis-homebrew-", dir=args.output_parent) as directory:
        root = Path(directory)
        package = args.package.resolve() if args.package else root / "ps4load.pkg"
        if not args.package:
            with urllib.request.urlopen(URL, timeout=120) as response, package.open("xb") as dest:
                remaining = 6619136
                while chunk := response.read(min(1024 * 1024, remaining + 1)):
                    remaining -= len(chunk)
                    if remaining < 0:
                        raise ValueError("Fixture download exceeds pinned size")
                    dest.write(chunk)
        if digest(package) != SHA256:
            raise ValueError("PS4Load fixture checksum mismatch")
        report = json.loads(run(binary, "verify", package, "--integrity", "--json", expected=3).stdout)
        assert report["summary"] == {"digests": 20, "macs": 92, "unavailable": 0, "failed": 0}, report
        assert report["readability"]["skipped"] == [{"path": "sce_sys/license.info", "reason": "no decryption key for slot 2"}], report
        output = root / "extracted"
        run(binary, "extract", package, "-o", output, "--resume", "-q", expected=3)
        run(binary, "extract", package, "-o", output, "--resume", "-q", expected=3)
        expected = json.loads(Path(__file__).with_name("ps4load-files.json").read_text())
        actual = {p.relative_to(output).as_posix(): digest(p) for p in output.rglob("*") if p.is_file() and not p.name.startswith("._")}
        assert actual == expected, "Extracted files differ from the checked-in 20-file baseline"
        print("PS4Load v0.5.0: 20 SHA-256 digests, 92 HMACs, 20 output hashes and resume verified; expected license.info omission.")


if __name__ == "__main__":
    main()
