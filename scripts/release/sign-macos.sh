#!/usr/bin/env bash
# Only invoked on a trusted master push with MACOS_SIGNING_ENABLED=true.
set -euo pipefail
binary=${1:?missing binary}
: "${MACOS_CERTIFICATE_BASE64:?missing Developer ID certificate}"
: "${MACOS_CERTIFICATE_PASSWORD:?missing certificate password}"
: "${MACOS_SIGNING_IDENTITY:?missing Developer ID Application identity}"
: "${APPLE_ID:?missing Apple ID}" "${APPLE_TEAM_ID:?missing Apple team ID}"
: "${APPLE_APP_PASSWORD:?missing Apple app-specific password}"
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/orbis-signing.XXXXXX")
keychain="$work_dir/signing.keychain-db"
keychain_password=$(openssl rand -hex 32)
cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -rf "$work_dir"
}
trap cleanup EXIT
umask 077
printf '%s' "$MACOS_CERTIFICATE_BASE64" | base64 --decode > "$work_dir/certificate.p12"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 3600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$work_dir/certificate.p12" -P "$MACOS_CERTIFICATE_PASSWORD" -T /usr/bin/codesign -t cert -f pkcs12 -k "$keychain" >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
codesign --force --keychain "$keychain" --sign "$MACOS_SIGNING_IDENTITY" --options runtime --timestamp "$binary"
codesign --verify --strict --verbose=2 "$binary"
# notarytool accepts ZIP, not tar.gz. Submit these exact signed executable bytes
# before packaging them into the final release archive. Mach-O tools cannot staple.
ditto -c -k "$binary" "$work_dir/submission.zip"
xcrun notarytool submit "$work_dir/submission.zip" --apple-id "$APPLE_ID" \
  --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait --timeout 30m \
  --output-format json > "$work_dir/notarization.json"
python3 - "$work_dir/notarization.json" <<'PY'
import json, sys
with open(sys.argv[1]) as stream:
    report = json.load(stream)
if report.get("status") != "Accepted":
    raise SystemExit(f"Notarization failed: {report.get('id')} ({report.get('status')})")
print(f"Notarization accepted: {report['id']}")
PY
