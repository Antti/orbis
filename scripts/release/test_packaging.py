"""Exercise the AppImage entry point without downloading a packaging tool."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class AppRunTests(unittest.TestCase):
    def test_launches_relative_to_appdir_and_preserves_cli_arguments(self):
        source = Path(__file__).resolve().parents[2] / "packaging" / "AppRun"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "an app with spaces"
            binary = root / "usr" / "bin" / "orbis-pkg-util"
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


if __name__ == "__main__":
    unittest.main()
