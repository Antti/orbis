"""Check launchable desktop archive contents without a display or credentials."""
import importlib.util
import os
from pathlib import Path
import plistlib
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("package_desktop", Path(__file__).with_name("package-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class DesktopPackagingTests(unittest.TestCase):
    def test_each_platform_contains_a_launchable_app_and_instructions(self):
        for target in desktop.TARGETS:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                binary = root / "a binary with spaces"
                binary.write_bytes(b"fixture executable")
                with patch.dict(os.environ, {"MACOS_NOTARIZED": "false"}):
                    archive = desktop.package("1.2.3", target, binary, root / "out")
                prefix = f"orbis-desktop-1.2.3-{target}"
                if target.endswith("windows-msvc"):
                    with zipfile.ZipFile(archive) as source:
                        self.assertEqual(source.read(f"{prefix}/orbis-desktop.exe"), binary.read_bytes())
                        self.assertIn(b"Double-click orbis-desktop.exe", source.read(f"{prefix}/START-HERE.txt"))
                        self.assertIn(f"{prefix}/LICENSE-MIT", source.namelist())
                        self.assertIn(b"SIL OPEN FONT LICENSE", source.read(f"{prefix}/licenses/Inter-OFL.txt"))
                else:
                    with tarfile.open(archive) as source:
                        suffix = "Orbis.app/Contents/MacOS/orbis-desktop" if target.endswith("apple-darwin") else "orbis-desktop"
                        executable = source.getmember(f"{prefix}/{suffix}")
                        self.assertTrue(executable.mode & 0o111)
                        self.assertEqual(source.extractfile(executable).read(), binary.read_bytes())
                        self.assertIn(f"{prefix}/START-HERE.txt", source.getnames())
                        licenses = "Orbis.app/Contents/Resources/licenses" if target.endswith("apple-darwin") else "licenses"
                        self.assertIn(b"SIL OPEN FONT LICENSE", source.extractfile(f"{prefix}/{licenses}/Inter-OFL.txt").read())
                        if target.endswith("apple-darwin"):
                            plist = plistlib.loads(source.extractfile(f"{prefix}/Orbis.app/Contents/Info.plist").read())
                            self.assertEqual(plist["CFBundleExecutable"], "orbis-desktop")
                            self.assertEqual(plist["CFBundleShortVersionString"], "1.2.3")
                            self.assertNotIn("CFBundleDocumentTypes", plist)

    def test_signing_failure_does_not_create_an_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "binary"
            binary.write_bytes(b"fixture")
            with patch.dict(os.environ, {"MACOS_NOTARIZED": "true"}), patch.object(desktop.subprocess, "run", side_effect=RuntimeError("signing failed")):
                with self.assertRaisesRegex(RuntimeError, "signing failed"):
                    desktop.package("1.2.3", "aarch64-apple-darwin", binary, root / "out")
            self.assertEqual(list((root / "out").iterdir()), [])


if __name__ == "__main__":
    unittest.main()
