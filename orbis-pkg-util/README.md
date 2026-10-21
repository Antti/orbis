# orbis-pkg-util

Inspect, extract, check, and install updates from PlayStation 4 PKG files. Run `orbis-pkg-util --help`
or add `--help` after any command for examples.

## Installation

From the repository root:

```sh
cargo install --path orbis-pkg-util
```

Prebuilt Linux and macOS packages and installation instructions are described in
[RELEASE.md](../RELEASE.md).

## Inspect a package

```sh
orbis-pkg-util info game.pkg
orbis-pkg-util info game.pkg --json
orbis-pkg-util info game.pkg --sfo
orbis-pkg-util info game.pkg --header-only
orbis-pkg-util list game.pkg
orbis-pkg-util list game.pkg --entries
orbis-pkg-util list game.pkg --files --json
```

`info` shows header metadata and an application summary from `param.sfo`: title,
application/package versions, category, required system version, content-ID region,
and available build fields such as SDK version. `--sfo` shows every typed SFO field.
`--json` includes all SFO fields, preserving integer values and encoding binary
values as hex. Region is inferred from the content ID, not a compatibility check.

`--header-only` skips SFO and key loading. Without it, unreadable or malformed SFO
produces a warning and exit 3 while retaining header output. Packages without an
SFO entry return a null application/SFO result. `content_version` remains the
content-ID version; `application.app_version` is the application's `APP_VER`.

`list` shows both PKG metadata and embedded filesystem contents. Paths match full
extraction destinations: metadata appears under `sce_sys/`. `--entries` lists
only the PKG entry table without requiring keys; `--files` lists only filesystem
files and directories. Known metadata IDs retain canonical paths. Unknown IDs
use safe filenames from the PKG name table, or `sce_sys/entry_XXXXXXXX.bin` when
unnamed. Invalid names/offsets are rejected; extraction refuses duplicate paths,
including case-only collisions, before publishing files.

If the filesystem cannot be read, the default listing still returns metadata,
prints a warning, and exits with code 3. Use `--entries` when only metadata is
wanted. `--files` fails with code 1 if the filesystem is unavailable.

## Extract all or selected files

```sh
orbis-pkg-util extract game.pkg
orbis-pkg-util extract game.pkg --output ./extracted
orbis-pkg-util extract game.pkg -o ./extracted --path textures --path eboot.bin
orbis-pkg-util extract game.pkg -o ./metadata --metadata-only --path sce_sys/param.sfo
orbis-pkg-util extract game.pkg -o ./extracted --force --quiet
```

The default destination is `./<TITLE_ID>` in the current directory, for example
`./CUSA03173`. Packages sharing a title ID therefore share a default destination;
use different `--output` directories to keep them separate.

`--path` selects an exact file or a directory and all its descendants. Repeat it
to select several paths. Paths use `/` on every platform, are relative to the
extraction root, and do not support globs, absolute paths, `.` or `..`. A missing
selection fails before writing files. `--metadata-only` avoids opening the
filesystem and can recover readable metadata from otherwise unsupported packages.

Existing files are preserved unless `--force` is supplied. Each file is staged
beside its destination and synced after a complete write. A
failed replacement preserves the previous file. Known output conflicts are
checked before writing; publication also checks for files created concurrently.
Archive-controlled paths remain confined to the output directory.

Linux, macOS, and Windows prefer exclusive rename when `--force` is absent.
Other platforms first try hard links. If the operation is unsupported, as with
macOS exFAT, extraction creates the destination exclusively and copies the completed
staging file into it. Existing files and symlinks are still refused. This fallback
adds a second copy and makes the destination visible during that copy. Normal
errors and Ctrl-C remove the unfinished copy, but forced termination or power loss
can leave a partial destination as well as staging files. Retry with `--force`
after checking the destination. Atomic publication remains in use where supported.
Staging temporarily uses space for both the old and replacement files, and
syncing each completed file can add I/O overhead.

On filesystems supporting rename publication, each file is published atomically;
the overall extraction is not atomic across the directory. Files completed
before a later failure remain. Ctrl-C cooperatively stops processing, discards
unfinished staged files, and exits 130. A process killed forcibly or a power
failure can leave `.orbis-*.tmp` staging files. After investigating a failure,
retry with `--force` to replace completed files safely.

To run the optional regression on a mounted filesystem, set
`ORBIS_TEST_EXTRACT_ROOT=/path/to/mount` and run
`cargo test -p orbis-pkg-util --lib extracts_to_a_configured_external_filesystem`.
It creates and removes only a private temporary subtree on that volume. The normal
test suite also injects unsupported-rename errors without requiring an external disk.

Missing decryption keys are reported as omissions. An incomplete extraction returns exit code 3 and identifies skipped entries;
`--quiet` does not hide these warnings.

## Check data without extracting it

```sh
orbis-pkg-util verify game.pkg
orbis-pkg-util verify game.pkg --metadata-only
orbis-pkg-util verify game.pkg --path textures
orbis-pkg-util verify game.pkg --integrity
orbis-pkg-util verify game.pkg --integrity --json > report.json
orbis-pkg-util verify game.pkg --integrity --metadata-only
```

`verify` reads selected metadata payloads and filesystem data, checks file lengths,
and decompresses compressed files. It creates no extracted files. Unknown entry
IDs use the same resolved/generated paths as extraction.
Unreadable encrypted entries are reported as skipped and return exit code 3.

By default this checks readability and decompression. `--integrity` additionally
checks stored SHA-256 hashes for the PKG header, body, SC entry chains, digest
table, individual stored entry payloads, and full/signed PFS regions. It also
checks the signed outer/inner PFS header, inode-table blocks, and file/directory
blocks through direct, single-indirect and double-indirect pointers. SHA-256
covers on-disk bytes, including ciphertext; HMACs cover decrypted PFS blocks.

Integrity checks apply to the whole selected scope and cannot use `--path`.
`--metadata-only` excludes PFS hashes/HMACs. Missing/all-zero hashes, unavailable
signing keys, contiguous signed extents without individual signatures, and
inode-table signature trees deeper than double-indirect return exit 3.
The inode-table descriptor supports S64 direct, single-indirect and
double-indirect pointers, independently of S32 data inode addressing.
A nonzero digest/MAC mismatch returns exit 1. These distinctions also apply with
`--quiet`. Ctrl-C interrupts both hashing and readability checks with exit 130.
Expect additional I/O: whole-image hashing and block verification read data
beyond the ordinary file checks. No extracted files are written.

Neither mode verifies publisher RSA signatures, licenses, executable behavior,
or publisher authenticity. Matching stored hashes does not establish trust.

## Integrity reports for scripts

`verify --integrity --json` streams one JSON document to stdout with
`schema_version: 1`, `package`, `metadata_only`, `checks`, `summary`, `status`,
`readability`, and `error`. It keeps the same exit codes: 0 complete, 1 failed,
3 incomplete, 130 interrupted. `--json` requires `--integrity`.

Each check contains:

| Field | Values / meaning |
| --- | --- |
| `layer` | `pkg`, `outer_pfs`, or `inner_pfs` |
| `scope` | Header, entry ID, inode or block location; descriptive text |
| `algorithm` | `sha256` or `hmac_sha256` |
| `status` | `passed`, `unavailable`, `failed`, or `not_applicable` |
| `reason` | `null`, `missing_digest`, `missing_key`, `unsigned_image`, `contiguous_extent`, `unsupported_layout`, or `digest_mismatch` |
| `count` | Checks represented; unavailable groups may exceed one; unsigned images use zero |

`summary` counts passed `digests`/`macs`, `unavailable` checks and `failed`
comparisons. A missing signing key is one unavailable image-level check, not an
estimate of its block count. The top-level `status` is `complete`, `incomplete`,
`failed`, or `interrupted`. `readability` contains `status`, `files`, `bytes`, and
`skipped` entries (`path` and `reason`) when that pass finishes; otherwise it is
`null`. `error` describes input, structural, I/O, mismatch or cancellation errors.
An error can therefore produce `status: "failed"` with zero failed comparisons.

Verification stops at the first mismatch or error; the report includes only
checks reached before stopping. It closes the JSON document on handled failures
and Ctrl-C, including input-open failures. A broken output stream or forced
termination cannot produce a complete document. Warnings can still go to stderr.
Check records are streamed so retaining the report does not use memory
proportional to the number of filesystem blocks. Missing checks are also grouped
by layer and reason in ordinary text output.

## Output for scripts

`info --json` and `list --json` emit one JSON document on stdout with
`schema_version: 1`. Progress, warnings, summaries, and errors go to stderr.
`--quiet` hides progress and success messages, while `--verbose` includes error
causes. These options can appear before or after the subcommand.

`info` returns `package` and a `metadata` object containing the content ID fields,
content/DRM types and names, flags, file and entry counts, sizes, and offsets.
Numeric sizes are bytes and offsets are numeric, not hexadecimal strings.
The additional `application` object contains the SFO summary, `sfo` maps keys to
`{"type":"string","value":...}`, `{"type":"integer","value":...}`, or
`{"type":"binary","hex":...}`, and `warnings` lists unavailable SFO details.
These additive fields retain schema version 1. The `package` display path replaces
invalid UTF-8 with the replacement character.

`list` returns `package`, `items`, and `warnings`. Each item has:

| Field | Meaning |
| --- | --- |
| `path` | Extraction-relative path, including resolved/generated unknown-entry names |
| `source` | `metadata` or `filesystem` |
| `kind` | `file` or `directory` |
| `size` | File bytes; zero for directories |
| `entry_id` | Numeric PKG entry ID; `null` for filesystem items |
| `entry_encrypted` | PKG entry encryption flag; `null` for filesystem items |
| `key_index` | Encrypted entry's key slot; otherwise `null` |

The entry encryption flag does not describe filesystem-sector encryption.
Listings are sorted by path and entry ID. Incomplete listings still produce valid
JSON and include warnings. A fatal error exits 1 without a JSON document.
Consumers should accept added fields within schema version 1.

| Exit code | Meaning |
| --- | --- |
| 0 | Requested operation completed |
| 1 | Input, data, or output failure |
| 2 | Invalid command-line arguments |
| 3 | Incomplete inspection, extraction, verification, or listing |
| 130 | Extraction or verification interrupted with Ctrl-C |

Closing a stdout pipe early, for example through `head`, exits without a panic.

## Shell completions

```sh
mkdir -p ~/.local/share/bash-completion/completions
orbis-pkg-util completions bash > ~/.local/share/bash-completion/completions/orbis-pkg-util

mkdir -p ~/.zfunc
orbis-pkg-util completions zsh > ~/.zfunc/_orbis-pkg-util

mkdir -p ~/.config/fish/completions
orbis-pkg-util completions fish > ~/.config/fish/completions/orbis-pkg-util.fish
```

For Bash, enable bash-completion or source the generated file. For Zsh, add
`fpath=(~/.zfunc $fpath)` before `autoload -Uz compinit; compinit` in `.zshrc`.
PowerShell and Elvish scripts are also available through `completions powershell`
and `completions elvish`.

## Encryption support

Header inspection and entry-table listing require no keys. By default extraction
uses the built-in entry key for slot 3 and the FPKG filesystem key scheme.

Provide a **known** package passcode to derive all seven entry keys:

```sh
orbis-pkg-util info game.pkg --passcode-file ./passcode.txt --json
orbis-pkg-util extract game.pkg --passcode-file ./passcode.txt -o ./game
orbis-pkg-util install update.pkg --base ./game --passcode-file ./passcode.txt
orbis-pkg-util verify game.pkg --integrity --passcode-file - < ./passcode.txt
```

The file/stdin must contain 32 printable ASCII characters, optionally followed
by LF or CRLF. Passcodes are checked against the package's stored key digests
before use; an incorrect passcode fails instead of falling back to a built-in
key. Passcodes are not included in JSON, diagnostics, or installation state.
Use a file with appropriate access permissions; no passcode value is accepted
on the command line. The library exposes `Pkg::load_passcode` for callers.

This does not provide arbitrary retail-PKG decryption or alternative RSA-key
import. Readable unencrypted metadata remains accessible when keys are missing.

## Install a full-file update

`install` targets an extracted game used by shadPS4. It creates a sibling
`<base>-UPDATE` directory; it does not modify the original base, create a new PKG,
or install anything onto a console.

```sh
# First extract the original base game, if needed.
orbis-pkg-util extract base.pkg -o ./games/CUSA12345

# Stop the emulator, inspect the plan, then install.
orbis-pkg-util install update.pkg --base ./games/CUSA12345 --dry-run
orbis-pkg-util install update.pkg --base ./games/CUSA12345 --dry-run --json
orbis-pkg-util install update.pkg --base ./games/CUSA12345

# Restore the previous update (or return to the base after the first install).
orbis-pkg-util rollback --base ./games/CUSA12345
```

Supported inputs must have PS4 content type `0x1A`, a cumulative-update flag
(`0x60000000`) or a first-update flag without the subsequent-update flag, and
`PARAM.SFO` category `gp`. The base must have category `gd`. Both must have matching
`TITLE_ID` and full `CONTENT_ID` strings and valid `APP_VER` values in `NN.NN`
format. The update version must be newer than the active version. Reinstalling
the same version and downgrades are refused; use `rollback` for the retained version.
Delta type `0x1E`, delta flags, unsupported classifications, missing identity fields,
and unsafe metadata paths are rejected. Unknown metadata is retained under
resolved/generated filenames. The existing FPKG key limitations apply.
No compatibility bypass is provided.

The entire update directory is replaced on upgrade. Files absent from the new
update disappear from the update layer; a same-named base file then becomes
visible again. This does not implement deletion instructions or binary deltas.
The planner reports `add`, `replace`, and `remove_from_update` operations; `replace`
indicates a path overlap, not a byte-for-byte content comparison. A sibling
`-patch` directory or an unmanaged `-UPDATE` directory causes refusal, so existing
manual installations are not silently adopted or removed.

`--dry-run` creates no files or directories and does not reserve disk space.
Its JSON has `schema_version: 1` and a `plan` containing identity, old/new versions,
destination, extracted byte count, estimated required/available space, and path
changes. The destination uses a lossy UTF-8 representation for non-UTF-8 host paths.
Planning walks the update filesystem but does not decompress every payload.
Installation rechecks the plan while holding an exclusive OS lock.

Installation stages and reads the complete update before activation. A SHA-256
inventory records the extracted files in `.orbis-install.json`; future installs
and rollbacks verify managed files and refuse locally modified content. These
hashes detect changes relative to the recorded installation; they do not establish
publisher authenticity. Reading/hashing the active update, rollback copy, and new
staging tree adds I/O. During an upgrade, space is needed for the new update while
the current update and its previous backup still exist. Only one rollback is
retained after a successful install.

### Interruption and recovery

Keep the emulator stopped and do not modify the base, update, or installation
state while an operation runs. Activation uses two directory renames and a
recovery journal; it is not an atomic exchange of directory trees. There can be
a brief interval with no active update directory.

Extraction failures and cooperative Ctrl-C discard staging and preserve the
active update. Ctrl-C returns 130 during cancellable work; once activation starts,
the short commit sequence completes before returning. An I/O failure before
commit attempts recovery automatically. Once journal removal commits the operation,
a failed final directory sync emits a warning (even with `--quiet`) and returns
success: the installed or restored version is already active, so do not retry the
operation. The warning means durability across power loss is uncertain.

After a process crash or forced termination:

```sh
orbis-pkg-util recover --base ./games/CUSA12345
```

An uncommitted journal restores the installation that existed before the
operation, including its rollback copy. Once committed, the new installation
remains active. Recovery is repeatable and also removes abandoned staging and
retired backups. If recovery fails, keep the state directory and address the
reported filesystem error before retrying. Do not delete the journal manually.

State lives beside the base in `.<base>.orbis-install/`, with the lock, journal,
staging tree, and previous update. Move this directory together with the base
and its `-UPDATE` sibling. A successful rollback consumes the one rollback slot.
Process interruption is tested on each platform in CI; storage-device failure
and power-loss durability are not guaranteed, particularly on Windows where
directory syncing is not available through this implementation.

## Export a merged game

Use `export` when a consumer needs the base and update in one directory, rather
than shadPS4's separate update layer:

```sh
# Install the desired update first, if it is not already installed.
orbis-pkg-util install update.pkg --base ./games/CUSA12345

# The output directory must not exist; its parent must already exist.
orbis-pkg-util export --base ./games/CUSA12345 -o ./merged-game
```

The command reads the original base and its managed sibling `-UPDATE` directory,
using the same base metadata requirements as `install` (a `CUSA` title ID and
`gd` category).
Update files replace matching base paths in the export; unchanged base files and
empty directories are retained. With no installed update, it exports the base
alone. It uses the current update only, not older rollback copies. Installer
manifests and state are excluded. Exported files are independent copies, so edits
to the export do not affect the originals. Future installs or rollbacks do not
change an existing export; export again to a new destination when needed.

Export verifies the installed update against its recorded SHA-256 inventory and
holds the installation lock while copying. It may create the sibling state/lock
directory when exporting a base that has never had an update. Keep the emulator
stopped and source files unchanged until it finishes; the lock coordinates Orbis
operations, not external file editors. Unmanaged or modified updates, pending
recovery, symlinks, special files, case collisions, and file/directory conflicts
are refused. The output must be outside the base, update, and installation state.

Allow space for the full merged game. Overridden base files are not copied. A
temporary sibling `.orbis-export-*.tmp` directory holds the export until every file
has been copied; publication refuses even an empty destination created meanwhile.
When exclusive directory rename is unsupported, including macOS exFAT, export
creates the destination exclusively and copies the completed staging tree into it.
Existing destinations are still refused, even empty directories. This fallback
adds a second full copy, needs temporary space for **two merged game copies**, and
makes the destination visible while copying. A separate publication progress phase
tracks that copy. Only planned game paths are copied; filesystem-generated metadata
sidecars in staging are not treated as additional game files.

Normal failures and Ctrl-C discard staging and attempt to remove partial output.
Cleanup preserves files or directories added or replaced by another process, so
these can leave a destination behind. Do not use or modify the export until the
command succeeds. Forced termination or power loss can leave staging and a partial
destination; after the exporter exits, inspect and remove abandoned output before
retrying, or choose a new destination. An error syncing the parent after
publication reports success with a durability warning because the export already
exists. Success messages go to stderr and `--quiet` suppresses them.

For an optional regression on a mounted filesystem, run
`ORBIS_TEST_EXPORT_ROOT=/path/to/mount cargo test -p orbis-pkg-util --lib exports_to_a_configured_external_filesystem`.
It creates and removes only a private temporary subtree on that volume. Portable
tests also inject unsupported-rename errors and exercise fallback cleanup.

For [bbport](https://github.com/deadinside28/bloodborne_pc), select the merged
directory in its launcher or set `BB_GAME_DIR` to its absolute path. bbport requires
the supported, clean Bloodborne 1.09 executable and decrypted game modules; consult
its current requirements. Export copies existing file contents: it does not decrypt
SELF executables, apply binary deltas, modify SFO values, or validate bbport's
executable hash. A successful export alone does not establish game compatibility.

The library exposes the same operations through `orbis_pkg_util::install`:
`update_metadata`, `plan`, `install`, `rollback`, and `recover`, with cancellable
planning and rollback variants. Progress reporters receive non-fatal commit
warnings through `ExtractProgress::warning`; use `recover_with_progress` to receive
them during recovery. The package passed to planning/installation must already
have any required entry and filesystem keys loaded.
