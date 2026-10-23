# Testing

Run the same checks used by GitHub Actions:

```sh
cargo test --workspace --all-features --locked
cargo test -p orbis-cli --no-default-features --locked
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
The shared fixture builders live in `orbis-cli/tests/common/`.

Desktop operation tests reuse these fixtures through the real library APIs.
They cover inspection, extraction, resume conflicts, incomplete results,
cancellation, update preview/install, and a changed update after preview.
Packaging tests verify launchable executables, macOS bundle metadata and file
permissions, and signing failures. See [desktop checks](orbis-desktop/README.md#development-checks)
for the native UI smoke tests that also need a graphical session. Linux workspace
builds now need `libxkbcommon-dev` and `libwayland-dev`; library/CLI-only builds do not.

The desktop app was smoke-tested on macOS arm64 with native file/folder pickers,
PS4Load inspection and extraction (20 files, one explicit missing-key omission),
and a synthetic 01.00-to-01.01 update preview/install. The base files stayed
unchanged and Show in folder opened the resulting update in Finder. Windows and
Linux native UI checks and signed macOS distribution still need their respective
environments; automated operation and packaging tests do not replace them.

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
exports also have the local real-package coverage described below; no Bloodborne
launch has been verified.

Synthetic fixtures do not replace compatibility checks against real packages.
Before a release, extract a known package and compare file contents with a trusted
reference extraction. For installation, use a representative original base plus
full-file update, compare the resulting update tree with a trusted extraction,
check the version in shadPS4, and exercise upgrade/rollback while the emulator is
stopped. The local validation below supplements the synthetic implementation tests.

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
target/release/orbis info sample.pkg --json > info.json
target/release/orbis verify sample.pkg --integrity --json > integrity.json
# Inspect status, errors, and skipped entries even if the command returns 3.
target/release/orbis extract sample.pkg --output ./sample-extracted
```

Compare extracted files against published source assets or an independent known
good extraction. Record package hashes, tool revision, exit status and omissions;
do not treat an incomplete result as a full pass. Use `--passcode-file` only when
the correct package passcode is known. Broader compatibility still requires more
representative packages and platforms.

## Real base/update validation (2026-10-23)

Local Bloodborne GOTY packages for `CUSA03173`, base `01.00` and cumulative
update `01.09`, were supplied on an exFAT USB drive for macOS arm64 validation.
Package bytes are not included in the repository. Sample SHA-256 values:

- Base, 31,351,111,680 bytes: `7d574cc0b0716755c093f3fbedee8d302d82d9894922c1972c12956c69b4d992`
- Update, 181,338,112 bytes: `a138cfdc6133d7718dca77a3ef05072d50020d8e987c21be8d795980f78ff818`

Both packages exposed a verifier bug at `sce_sys/npbind.dat` (entry `0x403`):
the entry declares 532 logical bytes, but its stored digest covers 544 bytes of
AES-aligned ciphertext. Independent Python SHA-256 checks confirmed both hashes
when including the padding. The verifier now hashes the padded encrypted range
and validates its bounds; unencrypted entries retain their exact lengths.
Synthetic regressions cover padding corruption, truncated padding even when a
digest is missing, plaintext alignment, and length overflow. The false mismatch
and missed-truncation tests failed before the fix.

The standard FPKG default passcode matched all seven stored entry-key digests.
Without it, base extraction explicitly omits `sce_sys/license.info` and returns
exit 3; supplying it recovers that metadata file. The update needs no passcode.

With the padding fix, full `verify --integrity --json` returned exit 0 for both:
the base passed 29 stored SHA-256 checks and 478,187 PFS HMAC checks; the update
passed 43 and 2,606 respectively. Readability and decompression passed with no
skipped files or unavailable checks. The base verification used the validated
default passcode. This establishes stored-data consistency, not authenticity.

Extraction and mutable workflows used disposable APFS destinations; USB inputs
and the existing game directory were left unchanged. The base extraction contains
28,831 files (31,513,388,912 bytes), and the update contains 442 files
(185,419,244 bytes). Separate installation reports `01.09` and its entire file
tree matches a standalone extraction of the update. The exported overlay contains
28,850 files (31,438,466,019 bytes); both manifest verification and an independent
Python comparison of all file SHA-256 values and directory paths passed.

Export resume passed. Separate-update rollback preserved the base inventory, and
the exported copy still passed manifest verification afterward. In-place merge
reported `01.09` and matched the same independently calculated overlay. Merge
rollback restored all original file hashes and directory paths exactly, including
the base `01.00` metadata. Status then reported no active update or recovery issues.

These checks exercise CLI/library behavior on macOS with APFS output. They do not
establish game launch compatibility, publisher authenticity, an independent
reference extraction, Windows/Linux behavior, or merge support on exFAT.

## Automated compatibility validation

The [compatibility workflow](.github/workflows/compatibility.yml) tests disposable
exFAT images on macOS/Linux, APFS, ext4, and pinned PS4Load extraction/integrity
on Linux/macOS/Windows. It checks output manifests and resume as well as copying.
See [validation scripts](scripts/validation/README.md) for local runs and the
opt-in matching base/update harness; incomplete real-package checks fail closed.

### In-place update merging

Synthetic merge tests exercise replacement and addition of files, exact restoration
of original inventories, refusal of mixed modes and repeat merges, incomplete
extraction, case/type conflicts, symlinks, altered backups, and tampered manifest
paths. Injected crashes cover journal publication, original backup, patch
publication, restoration, and commit. Recovery is repeated to check idempotence;
changed files are preserved by refusing restoration. CLI tests cover `--merge`
preview/install/status/rollback. Desktop operation tests cover mode selection and
restore/recovery without the original PKG.

For manual QA, merge into a disposable original base with the emulator closed.
Check the preview destination and retained-original size, merge, reopen the app,
then use Manage game and Restore base. Compare the restored directory to the
original. Test on each supported OS; macOS exFAT should refuse merging before
changing the game. These fixtures do not establish the semantics of omitted
files in successive real patches or guarantee power-loss durability.
