#!/usr/bin/env python3
"""Opt-in validation of local matching base/update PKGs. Never modifies input files."""
import argparse
import json
from pathlib import Path
import subprocess
from public_package import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("base", type=Path)
    parser.add_argument("update", type=Path)
    parser.add_argument("work", type=Path, help="New directory on a volume with space for extraction and export")
    parser.add_argument("--passcode-file", type=Path)
    args = parser.parse_args()
    binary, base_pkg, update_pkg = (p.resolve() for p in (args.binary, args.base, args.update))
    args.work.mkdir()  # Refuse an existing working directory.
    root = args.work.resolve()
    checks = []

    def run(*command):
        argv = [str(binary), *map(str, command)]
        if args.passcode_file:
            argv += ["--passcode-file", str(args.passcode_file.resolve())]
        result = subprocess.run(argv, capture_output=True, text=True)
        checks.append({"command": command[0], "exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr})
        (root / "validation.json").write_text(json.dumps(checks, indent=2) + "\n")
        if result.returncode != 0:
            raise RuntimeError(f"{command[0]} failed or was incomplete; inspect {root / 'validation.json'}")

    for package in (base_pkg, update_pkg):
        run("verify", package, "--integrity", "--json")
    base = root / "base"
    run("extract", base_pkg, "-o", base)
    run("install", update_pkg, "--base", base, "--dry-run", "--json")
    run("install", update_pkg, "--base", base)
    run("status", "--base", base, "--json")
    run("export", "--base", base, "-o", root / "merged", "--resume")
    run("verify-dir", root / "merged", "--json")
    def inventory(directory):
        return {p.relative_to(directory).as_posix(): digest(p) for p in directory.rglob("*") if p.is_file() and p.name not in (".orbis-install.json", ".orbis-export.json") and not p.name.startswith("._")}
    expected = inventory(base) | inventory(root / "base-UPDATE")
    assert inventory(root / "merged") == expected, "Merged output differs from independent base/update overlay"
    run("rollback", "--base", base)
    run("status", "--base", base, "--json")
    run("verify-dir", root / "merged", "--json")
    print(f"Pair validated; retained inputs/exports and report at {root}")


if __name__ == "__main__":
    main()
