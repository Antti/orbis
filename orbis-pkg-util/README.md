# orbis-pkg-util

Inspect, extract, and check PlayStation 4 PKG files. Run `orbis-pkg-util --help`
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
orbis-pkg-util list game.pkg
orbis-pkg-util list game.pkg --entries
orbis-pkg-util list game.pkg --files --json
```

`info` reads header metadata without requiring decryption keys. Sizes include
human-readable units; `content_version` is the version field in the content ID,
not the application's version from `param.sfo`.

`list` shows both PKG metadata and embedded filesystem contents. Paths match full
extraction destinations: metadata appears under `sce_sys/`. `--entries` lists
only the PKG entry table without requiring keys; `--files` lists only filesystem
files and directories. Unknown metadata IDs have no known extraction path.

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
beside its destination, synced, and published only after a complete write. A
failed replacement preserves the previous file. Known output conflicts are
checked before writing; publication also checks for files created concurrently.
Archive-controlled paths remain confined to the output directory.

Linux, macOS, and Windows use exclusive rename when `--force` is absent.
Other platforms use hard links and require filesystem support for them.
Staging temporarily uses space for both the old and replacement files, and
syncing each completed file can add I/O overhead.

An operation is atomic per file, not across the entire directory. Files completed
before a later failure remain. Ctrl-C cooperatively stops processing, discards
unfinished staged files, and exits 130. A process killed forcibly or a power
failure can leave `.orbis-*.tmp` staging files. After investigating a failure,
retry with `--force` to replace completed files safely.

Missing decryption keys and unknown entry IDs are reported as omissions. An
incomplete extraction returns exit code 3 and identifies skipped entries;
`--quiet` does not hide these warnings.

## Check data without extracting it

```sh
orbis-pkg-util verify game.pkg
orbis-pkg-util verify game.pkg --metadata-only
orbis-pkg-util verify game.pkg --path textures
```

`verify` reads selected metadata payloads and filesystem data, checks file lengths,
and decompresses compressed files. It creates no extracted files. Unknown entry
IDs can still be checked because verification does not require an output filename.
Unreadable encrypted entries are reported as skipped and return exit code 3.

This is a readability and decompression check. It does **not** validate PKG/PFS
signatures, MACs, stored digest tables, licenses, executable contents, or
publisher authenticity. A successful check does not establish that the package
is trusted.

## Output for scripts

`info --json` and `list --json` emit one JSON document on stdout with
`schema_version: 1`. Progress, warnings, summaries, and errors go to stderr.
`--quiet` hides progress and success messages, while `--verbose` includes error
causes. These options can appear before or after the subcommand.

`info` returns `package` and a `metadata` object containing the content ID fields,
content/DRM types and names, flags, file and entry counts, sizes, and offsets.
Numeric sizes are bytes and offsets are numeric, not hexadecimal strings.
The `package` display path replaces invalid UTF-8 with the replacement character.

`list` returns `package`, `items`, and `warnings`. Each item has:

| Field | Meaning |
| --- | --- |
| `path` | Extraction-relative path, or `null` for an unknown metadata ID |
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
| 3 | Incomplete extraction, verification, or listing |
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

Header inspection and entry-table listing require no keys. Extraction supports
the built-in entry key for slot 3 and the FPKG filesystem key scheme. It does not
provide arbitrary retail-PKG decryption or options to import alternative keys.
Readable unencrypted metadata remains accessible when other keys are unavailable.
