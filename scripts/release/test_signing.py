"""Exercise signing failure boundaries without Apple credentials or keychain changes."""
import base64
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class SigningTests(unittest.TestCase):
    def test_acceptance_is_required_and_temporary_keychains_are_cleaned(self):
        script = Path(__file__).with_name("sign-macos.sh").resolve()
        for status, signature_failure in [("Accepted", False), ("Invalid", False), ("Accepted", True)]:
            with self.subTest(status=status, signature_failure=signature_failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                tools = root / "tools"
                tools.mkdir()
                log = root / "commands"
                for name in ("security", "openssl", "codesign", "ditto", "xcrun"):
                    tool = tools / name
                    tool.write_text(
                        '#!/bin/sh\n'
                        'name=${0##*/}\n'
                        'printf "%s %s\\n" "$name" "$1" >> "$FAKE_LOG"\n'
                        'case "$name" in\n'
                        'openssl) printf "temporary-password\\n" ;;\n'
                        'codesign) if [ "$FAKE_BAD_SIGNATURE" = true ]; then exit 1; fi ;;\n'
                        'xcrun) printf \'{"status":"%s","id":"test-submission"}\\n\' "$FAKE_STATUS" ;;\n'
                        'esac\n'
                    )
                    tool.chmod(0o755)
                env = dict(os.environ, PATH=f"{tools}{os.pathsep}{os.environ['PATH']}", TMPDIR=str(root),
                           FAKE_LOG=str(log), FAKE_STATUS=status, FAKE_BAD_SIGNATURE=str(signature_failure).lower(),
                           MACOS_CERTIFICATE_BASE64=base64.b64encode(b"test fixture").decode(),
                           MACOS_CERTIFICATE_PASSWORD="test-password", MACOS_SIGNING_IDENTITY="test-identity",
                           APPLE_ID="test@example.invalid", APPLE_TEAM_ID="test-team", APPLE_APP_PASSWORD="test-password")
                result = subprocess.run(["bash", str(script), "fixture-binary"], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, status == "Accepted" and not signature_failure, result.stderr)
                self.assertIn("security delete-keychain", log.read_text())
                self.assertEqual(list(root.glob("orbis-signing.*")), [])
                if signature_failure:
                    self.assertNotIn("xcrun notarytool", log.read_text())


if __name__ == "__main__":
    unittest.main()
