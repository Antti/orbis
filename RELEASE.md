# Releases

The `Release packages` workflow publishes a GitHub release when
`orbis-pkg-util/Cargo.toml` increases its `package.version` on `master`.
The version must be a stable `major.minor.patch` value; prereleases are not
currently supported. Unchanged versions do not publish, and decreases fail
validation.

Each release contains:

| Asset | Platform |
| --- | --- |
| `orbis-pkg-util-VERSION-linux-x86_64.AppImage` | Linux x86_64, static musl binary |
| `orbis-pkg-util-VERSION-linux-x86_64.flatpak` | Linux x86_64, Freedesktop 25.08 runtime |
| `orbis-pkg-util-VERSION-x86_64-apple-darwin.tar.gz` | Intel Mac, macOS 11 or later |
| `orbis-pkg-util-VERSION-aarch64-apple-darwin.tar.gz` | Apple Silicon Mac, macOS 11 or later |
| `orbis-pkg-util-VERSION-linux-aarch64.AppImage` | Linux ARM64, static musl binary |
| `orbis-pkg-util-VERSION-linux-aarch64.flatpak` | Linux ARM64, Freedesktop 25.08 runtime |
| `orbis-pkg-util-VERSION-x86_64-pc-windows-msvc.zip` | Windows x86-64 |
| `SHA256SUMS` | SHA-256 checksums for all seven downloads |

The packages contain the CLI and both project licenses. Mac archives include
`BUILD-INFO.txt` stating whether the binary was signed and notarized. Builds are
unsigned until the optional signing configuration below is enabled; macOS may
require explicit approval for unsigned binaries. Windows binaries are unsigned.
Flatpak bundles are unsigned and distributed through GitHub, not Flathub.
Flathub supplies only the runtime. There is no automatic update repository for
the application.

## Cutting a release

1. Increase the CLI package version in `orbis-pkg-util/Cargo.toml`. If the
   workspace dependency on `orbis-pkg-util` in the root `Cargo.toml` no longer
   accepts that version, update its requirement too.
2. Run `cargo check --workspace` to refresh `Cargo.lock`, and commit the lockfile
   with the version change. Library versions only need changes when those crates
   are released separately.
3. Open a PR and inspect the `Release packages` checks and downloadable workflow
   artifacts. The workflow tests optimized builds and runs each packaged CLI
   with `--help`. Check a representative real PKG before merging.
4. Merge to `master`. A version increase across the entire push triggers the
   build, creates tag `vVERSION` at that exact commit, uploads all assets to a
   draft, and publishes it after the uploads finish.

Pull requests and manual workflow runs build packages without creating tags or
releases. This also allows packaging changes to be reviewed before any version
is bumped. No release is created merely by adding this workflow.

If a release fails, rerun the failed workflow from its original push. A retry can
finish the draft and replace its incomplete uploads. It refuses a tag belonging
to another commit and leaves an already published release unchanged. If a
published release needs correction, bump the version again. Do not move its tag.
To disable future publication, disable `Release packages` in Actions.

The repository must allow GitHub Actions and the workflow's `contents: write`
permission. Only the publishing job has that permission; PR builds use read-only
permissions. Unsigned releases require no third-party service secrets.

## Installing packages

Replace `VERSION` with the downloaded release version and verify checksums first:

```sh
# Linux; on macOS use: shasum -a 256 -c SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
```

AppImage (replace `x86_64` with `aarch64` for Linux ARM64):

```sh
chmod +x orbis-pkg-util-VERSION-linux-x86_64.AppImage
./orbis-pkg-util-VERSION-linux-x86_64.AppImage --help
```

If FUSE is unavailable, add `--appimage-extract-and-run` before the CLI arguments.

Flatpak:

```sh
flatpak install --user ./orbis-pkg-util-VERSION-linux-x86_64.flatpak
flatpak run io.github.Antti.Orbis --help
mkdir -p "$HOME/Orbis"
flatpak run --filesystem="$HOME/Orbis" io.github.Antti.Orbis \
  extract "$HOME/Orbis/game.pkg" --output "$HOME/Orbis/extracted"
```

The Flatpak has no network or host filesystem access by default. Grant access to
the input/output directory explicitly as above. Downloads of the runtime require
network access during installation.

macOS:

```sh
# Choose aarch64 for Apple Silicon or x86_64 for Intel.
tar -xzf orbis-pkg-util-VERSION-aarch64-apple-darwin.tar.gz
./orbis-pkg-util-VERSION-aarch64-apple-darwin/orbis-pkg-util --help
```

Windows (PowerShell):

```powershell
Get-FileHash .\orbis-pkg-util-VERSION-x86_64-pc-windows-msvc.zip -Algorithm SHA256
# Compare the result with the corresponding line in SHA256SUMS.
Expand-Archive .\orbis-pkg-util-VERSION-x86_64-pc-windows-msvc.zip -DestinationPath .
.\orbis-pkg-util-VERSION-x86_64-pc-windows-msvc\orbis-pkg-util.exe --help
```

## Optional macOS signing and notarization

Configure these repository secrets through GitHub Settings → Secrets and variables
→ Actions. Never commit certificate material or passwords:

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE_BASE64` | Base64-encoded Developer ID Application `.p12`, including its private key |
| `MACOS_CERTIFICATE_PASSWORD` | Password protecting that `.p12` |
| `MACOS_SIGNING_IDENTITY` | Full Developer ID Application identity from the certificate |
| `APPLE_ID` | Apple Developer account email |
| `APPLE_TEAM_ID` | Developer team ID |
| `APPLE_APP_PASSWORD` | App-specific password for notarization |

After all secrets are configured, set repository variable `MACOS_SIGNING_ENABLED`
to `true`, then release a new version. Signing runs only on `master` pushes;
pull requests and manual builds remain unsigned. Missing credentials, invalid
signatures, or rejected notarization fail the release instead of silently
publishing unsigned assets. Remove/set the variable to `false` to return to
unsigned releases; published assets remain immutable.

The workflow imports the certificate into a temporary keychain, enables hardened
runtime, timestamps the signature, verifies it, and waits for Apple to accept
notarization before archiving. The keychain is removed on exit. A standalone CLI
cannot carry a stapled ticket; Gatekeeper needs network access to retrieve it.
Follow [Apple's notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
for certificate and account setup. Signed release execution cannot be validated
without these credentials; unsigned PR builds still smoke-test both Mac targets.

## Maintaining the automation

`scripts/release/version.py` compares the pre-push commit with the checked-out
manifest; it does not depend on commit messages or a separate version file.
`publish.py` requires exactly seven nonempty assets before writing checksums.
Their tests run with Python 3.11 or later:

```sh
python3 -m unittest discover -s scripts/release -p 'test_*.py' -v
shellcheck scripts/release/*.sh packaging/AppRun
```

The Linux script uses checksum-pinned appimagetool 1.9.1 and AppImage runtime
20251108. Update their URLs and SHA-256 values together when upgrading. Review the
Freedesktop runtime version before its support ends. Native macOS jobs build and
test on both architectures; Linux packaging requires Flatpak, musl tools, curl,
and D-Bus. GitHub-hosted Ubuntu 22.04 runners provide the user namespaces needed
by the Flatpak smoke test.
