"""Exercise CLI archive names, compatibility commands, and the AppImage launcher."""

import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("package_windows", Path(__file__).with_name("package-windows.py"))
windows = importlib.util.module_from_spec(spec)
spec.loader.exec_module(windows)


class AppRunTests(unittest.TestCase):
    def test_launches_relative_to_appdir_and_preserves_cli_arguments(self):
        source = Path(__file__).resolve().parents[2] / "packaging" / "AppRun"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "an app with spaces"
            binary = root / "usr" / "bin" / "orbis"
            binary.parent.mkdir(parents=True)
            binary.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
            binary.chmod(0o755)
            launcher = root / "AppRun"
            shutil.copyfile(source, launcher)
            args = ["extract", "a package.pkg", "--output", "some directory", ""]
            for use_env in [False, True]:
                with self.subTest(use_env=use_env):
                    env = os.environ.copy()
                    env.pop("APPDIR", None)
                    if use_env:
                        env["APPDIR"] = str(root)
                    result = subprocess.run(
                        ["sh", str(launcher), *args],
                        env=env, cwd=directory, text=True, capture_output=True, check=True,
                    )
                    self.assertEqual(result.stdout, "\n".join(args) + "\n")


class CliArchiveTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix", "macOS packager uses a POSIX shell")
    def test_macos_archive_keeps_an_executable_compatibility_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "binary with spaces"
            binary.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
            binary.chmod(0o755)
            result = subprocess.run(
                ["bash", str(ROOT / "scripts/release/package-macos.sh"),
                 "1.2.3", "aarch64-apple-darwin", str(binary), str(root / "out")],
                cwd=ROOT, env={**os.environ, "MACOS_NOTARIZED": "false"},
                capture_output=True, text=True, check=True,
            )
            # Original, unpacked canonical, and unpacked compatibility commands.
            self.assertEqual(result.stdout.splitlines(), ["--help"] * 3)
            prefix = "orbis-cli-1.2.3-aarch64-apple-darwin"
            with tarfile.open(root / "out" / f"{prefix}.tar.gz") as archive:
                executable = archive.getmember(f"{prefix}/orbis")
                self.assertTrue(executable.mode & 0o111)
                self.assertEqual(archive.extractfile(executable).read(), binary.read_bytes())
                alias = archive.getmember(f"{prefix}/orbis-pkg-util")
                self.assertTrue(alias.issym())
                self.assertEqual(alias.linkname, "orbis")

    def test_windows_archive_contains_both_command_names(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "source.exe"
            binary.write_bytes(b"fixture executable")
            args = ["package-windows.py", "1.2.3", str(binary), str(root / "out")]
            with patch("sys.argv", args), patch.object(windows.subprocess, "run") as run:
                windows.main()
            prefix = "orbis-cli-1.2.3-x86_64-pc-windows-msvc"
            with zipfile.ZipFile(root / "out" / f"{prefix}.zip") as archive:
                for name in ["orbis.exe", "orbis-pkg-util.exe"]:
                    self.assertEqual(archive.read(f"{prefix}/{name}"), binary.read_bytes())
                self.assertIn(f"{prefix}/LICENSE-MIT", archive.namelist())
            self.assertEqual(
                [Path(call.args[0][0]).name for call in run.call_args_list],
                ["source.exe", "orbis.exe", "orbis-pkg-util.exe"],
            )


if __name__ == "__main__":
    unittest.main()
