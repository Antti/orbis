# Testing

Run the same checks used by GitHub Actions:

```sh
cargo test --workspace --all-features --locked
cargo test -p orbis-pkg-util --no-default-features --locked
cargo test --workspace --all-features --release --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

CI runs the debug tests on Linux, macOS, and Windows, and release tests on Linux.
Release tests matter because integer overflow behaves differently between debug
and optimized builds.

The tests construct small synthetic PKG, PFS, and PFSC images. They cover malformed
headers, block reads, decompression, extraction boundaries, and overwrite policy
without downloading game data. Extraction tests write only into temporary
directories. Unix-specific symlink tests run on Linux and macOS.

Library README examples are included in doctests. CLI integration tests run
synthetic packages through metadata inspection, filesystem listing, JSON output,
selected extraction, verification, completion generation, and failure handling.
They check exit codes and stdout/stderr separately, including warnings in quiet
mode and closed-pipe handling on Unix. Library tests cover atomic replacement,
concurrent destination creation, cancellation, and cleanup after corrupt input.
The shared fixture builders live in `orbis-pkg-util/tests/common/`.

Installer tests encode SFO identity/version metadata independently of the parser
and build synthetic base/update pairs. They cover read-only planning, delta and
identity rejection, complete update replacement, one-level rollback, modified or
unmanaged directories, concurrent operation locks, cancellation, corrupt payloads,
and recovery after simulated process exits at each persisted activation/rollback
boundary. Recovery is repeated to check idempotency. CLI tests verify dry-run JSON,
quiet installation, rollback, and actionable recovery errors. These tests run
through the existing Linux, macOS, and Windows jobs; library-only builds include
the installer as well. A wide-tree inventory regression uses a subprocess capped
at 256 file descriptors on Unix, without changing parallel tests' resource limits.
Injected failures of the final directory sync verify successful install, rollback,
and recovery outcomes with warnings; quiet console output retains these warnings.

Merged export regressions cover update precedence, base fallback, empty directories,
base-only exports, independent copies, metadata exclusion, preserved rollback,
managed-state validation, path conflicts, cancellation, copy failures, and a
destination created during copying. CLI tests check the reported version, quiet
output, argument errors, and shell completion. They run in the existing platform
matrix and library-only checks without additional dependencies.
Directory-identity checks also cover case aliases on case-insensitive volumes.
The available PS4Load homebrew extraction could not exercise a real export: its
`LOAD00044` title ID is outside the installer's supported `CUSA` identities. Merged
exports have synthetic base/update coverage, not a verified Bloodborne launch.

Synthetic fixtures do not replace compatibility checks against real packages.
Before a release, extract a known package and compare file contents with a trusted
reference extraction. For installation, use a representative original base plus
full-file update, compare the resulting update tree with a trusted extraction,
check the version in shadPS4, and exercise upgrade/rollback while the emulator is
stopped. No real base/update pair was available during implementation.

## Fuzzing

[Parser fuzzing](fuzz/README.md) covers PKG/SFO, name tables and integrity ranges,
PFS directory/file/signature trees, and PFSC decompression using cargo-fuzz and
AddressSanitizer. `.github/workflows/fuzz.yml` runs four bounded smoke targets on
PRs and master pushes, plus longer weekly runs. The stable regression workflow
also checks formatting in the separate fuzz workspace.

Integrity regressions independently encode signed S32 data and S64 inode-table
single/double-indirect trees. Tests check corrupted data, redirected descriptors,
missing signatures/keys, observer write errors, cancellation, and complete JSON
on successful, incomplete, malformed and mismatched packages.

## Homebrew compatibility validation (2026-10-20)

Two publicly released homebrew packages were checked on macOS arm64. Neither
package is bundled in the repository or downloaded by CI.

| Sample | Stored SHA-256 checks | PFS HMAC checks | Readable files | Outcome |
| --- | ---: | ---: | ---: | --- |
| [PS4Load v0.5.0](https://github.com/bucanero/ps4load/releases/tag/v0.5.0) | 20 | 92 | 20 | Exit 3: `sce_sys/license.info` needs entry key slot 2 |
| [ezRemote Client 2.08](https://github.com/cy33hc/ps4-ezremote-client/releases/tag/2.08) | 20 | 797 | 85 | Exit 3: the same entry key is unavailable |

All stored hashes checked in both samples matched. Readability and decompression
passed for the remaining files; the encrypted metadata omission is preserved in
JSON and text output. These samples exercise encrypted signed outer PFS and
unsigned inner PFS. Their results do not establish retail-package compatibility,
base/update installation, publisher authenticity, or real-world coverage of
large indirect inode tables; those layouts currently have synthetic coverage.

The PS4Load sample exposed reversed PFS inode size fields in the reader and its
fixtures. Offset `0x08` is the stored PFSC container length; `0x10` is the decoded
length. The reader now uses the stored size for reads and block-map bounds, and
the decoded size for `File::len`. Tests cover containers both smaller and larger
than the decoded data. This matches the independent
[reference reader's length handling](https://github.com/maxton/LibOrbisPkg/blob/master/LibOrbisPkg/PFS/PfsReader.cs).

Downloaded package SHA-256 values:

- PS4Load, 6,619,136 bytes: `00efee11c26404d2b6f350bcfd368c448fd0aa4863b74cd81216e4345092357e`
- ezRemote, 52,822,016 bytes: `8c05370922547e3b33a55338d67e6845e59dae22592ca1d89d31d632813443be`

PS4Load was also extracted into a fresh temporary directory. Two extracted assets
matched the corresponding files published at tag `v0.5.0`, byte for byte:

- [`assets/fonts/Gontserrat-Regular.ttf`](https://github.com/bucanero/ps4load/blob/v0.5.0/assets/fonts/Gontserrat-Regular.ttf): `e5d0395c7e0a3f43846ac532e1d054ee0c8b964ba7b715096b0d424d97e7aa3d`
- [`assets/images/psl1ght.tga`](https://github.com/bucanero/ps4load/blob/v0.5.0/assets/images/psl1ght.tga): `159cfced73b82082f660f8f76e39756af881c3be728406de2eaeb3c9c0264d62`

To repeat with these or locally owned packages, download the entire file first
and keep it unchanged while mapped. Use an explicit, fresh output directory for
homebrew IDs that do not use the `CUSA` convention:

```sh
cargo build --release --locked
# Replace sample.pkg with the downloaded package path.
target/release/orbis-pkg-util info sample.pkg --json > info.json
target/release/orbis-pkg-util verify sample.pkg --integrity --json > integrity.json
# Inspect status, errors, and skipped entries even if the command returns 3.
target/release/orbis-pkg-util extract sample.pkg --output ./sample-extracted
```

Compare extracted files against published source assets or an independent known
good extraction. Record package hashes, tool revision, exit status and omissions;
do not treat an incomplete result as a full pass. Use `--passcode-file` only when
the correct package passcode is known. A real base/update pair and large retail
packages are still needed before claiming broader compatibility.
