# Orbis: PS4 PKG extractor and update installer

[![GitHub release](https://img.shields.io/github/v/release/Antti/orbis)](https://github.com/Antti/orbis/releases/latest)
[![CI](https://github.com/Antti/orbis/actions/workflows/ci.yml/badge.svg)](https://github.com/Antti/orbis/actions/workflows/ci.yml)

Orbis is a **PS4 PKG extractor** and command-line tool written in Rust for
**Windows, Linux, and macOS**. Extract PlayStation 4 packages, inspect `PARAM.SFO`
metadata, verify package integrity, install supported game updates for shadPS4,
and export a merged base game and update into one directory.

The `orbis-pkg-util` CLI is backed by Rust libraries for reading PS4 PKG archives
and PFS (PlayStation File System) images. This repository extends the original
[videobitva/orbis Rust project](https://github.com/videobitva/orbis) with update
management, merged exports, integrity checks, and additional platform builds.

**[Download Orbis](https://github.com/Antti/orbis/releases/latest)** ·
[Installation instructions](RELEASE.md#installing-packages) ·
[CLI reference](orbis-pkg-util/README.md)

## What can Orbis do?

- **Extract PS4 PKG files:** unpack a whole package or selected files and directories.
- **Inspect packages:** list their contents and read title IDs, application versions,
  firmware requirements, and typed `PARAM.SFO` fields, including JSON output.
- **Check integrity:** verify readability, decompression, stored SHA-256 hashes,
  and supported PFS HMACs without extracting files.
- **Install full-file updates:** maintain shadPS4's sibling `-UPDATE` directory,
  keep the base intact, and retain one previous installation for rollback.
- **Export merged games:** copy the base and current update into a standalone
  directory with a SHA-256 manifest for later verification.
- **Resume interrupted work:** verify existing files before reusing them during
  extraction or export. Copy exports to exFAT USB drives as well as native filesystems.

## Download and install

Download binaries from [GitHub Releases](https://github.com/Antti/orbis/releases/latest).
The release workflow builds these packages:

| Platform | Architecture | Package |
| --- | --- | --- |
| Windows | x86-64 | ZIP containing `orbis-pkg-util.exe` |
| Linux | x86-64 | AppImage or Flatpak |
| Linux | ARM64 / aarch64 | AppImage or Flatpak |
| macOS | Intel x86-64 | `.tar.gz` archive |
| macOS | Apple Silicon ARM64 | `.tar.gz` archive |

Check the assets attached to your chosen release. **v0.2.0 predates Windows and
Linux ARM64 downloads and the export/resume commands documented here**; those
changes are on `master` for the next release. To use them now, build from source
with [Rust](https://www.rust-lang.org/tools/install):

```sh
cargo install --git https://github.com/Antti/orbis.git --locked orbis-pkg-util
```

This installs the CLI from this repository. The crates.io package can differ.
See the [platform installation guide](RELEASE.md#installing-packages) for checksum
verification, running downloaded executables, and Flatpak filesystem access.

## Quick start: extract a PS4 PKG

After installing the CLI:

```sh
# Read the game title, version, and package information.
orbis-pkg-util info game.pkg

# List metadata entries and files inside the package.
orbis-pkg-util list game.pkg

# Extract into a new directory.
orbis-pkg-util extract game.pkg --output ./extracted-game
```

The examples assume `orbis-pkg-util` is on your `PATH`. For an unpacked Windows
ZIP, use `./orbis-pkg-util.exe` in PowerShell from the executable's directory.
Downloaded AppImage and macOS archive commands are in the installation guide.

Without `--output`, extraction uses the title ID as the directory name. Existing
files are preserved unless you explicitly use `--force`. `--resume` checks
existing contents and copies missing files; it refuses conflicting files.

Select a file, inspect metadata as JSON, or verify stored integrity values:

```sh
orbis-pkg-util extract game.pkg -o ./metadata --metadata-only --path sce_sys/param.sfo
orbis-pkg-util info game.pkg --json
orbis-pkg-util verify game.pkg --integrity
```

`verify` without `--integrity` checks readability and decompression. Missing keys
or unavailable integrity checks produce exit code **3** (incomplete). See [CLI exit codes](orbis-pkg-util/README.md#exit-codes)
and [encryption support](orbis-pkg-util/README.md#encryption-support).

## Install a PS4 update for shadPS4

Start with an extracted base game. Stop the emulator, preview the update, then
install it:

```sh
orbis-pkg-util extract base.pkg -o ./games/CUSA12345
orbis-pkg-util install update.pkg --base ./games/CUSA12345 --dry-run
orbis-pkg-util install update.pkg --base ./games/CUSA12345
orbis-pkg-util status --base ./games/CUSA12345
```

The base remains in `CUSA12345`; the managed update lives beside it in
`CUSA12345-UPDATE`. The update must match the base title and content ID and have a
newer application version. Supported full-file updates replace the update layer;
binary delta patches are rejected.

To restore the retained installation:

```sh
orbis-pkg-util rollback --base ./games/CUSA12345
```

See the [update installation guide](orbis-pkg-util/README.md#install-a-full-file-update)
for supported package types, space requirements, and crash recovery.

## Merge a base game and update into one directory

Use `export` for software that expects update files inside the base game tree:

```sh
# Install the desired update first, using the commands above.
orbis-pkg-util export --base ./games/CUSA12345 -o ./merged-game --dry-run
orbis-pkg-util export --base ./games/CUSA12345 -o ./merged-game --resume
orbis-pkg-util verify-dir ./merged-game
```

Update files take precedence at matching paths; unchanged files come from the
base. The export contains independent copies and a `.orbis-export.json` manifest.
The original base, update, and rollback remain available. Repeating the resume
command verifies completed files before copying missing ones.

The output must be outside the source directories, and its parent must already
exist. A resumable export is visible while incomplete; use it only after success.
See [merged exports](orbis-pkg-util/README.md#export-a-merged-game) for exFAT behavior,
space requirements, and consumer-specific requirements such as bbport/Bloodborne.

## Package support and common questions

### Does Orbis extract FPKG files?

Orbis supports the PS4 fake-package (FPKG) filesystem key scheme and includes the
entry key for slot 3. A known package passcode can derive all seven entry keys.
It cannot decrypt arbitrary retail PKGs; entries without usable keys are reported
as skipped. Readable metadata can still be inspected. See
[PKG encryption and passcodes](orbis-pkg-util/README.md#encryption-support).

### Does extracting an update merge it with the base game?

`extract update.pkg` writes the update's own files. Use `install` to manage an
update alongside a matching base, then `export` when you need a single merged
directory. Export copies existing contents; it does not decrypt SELF executables,
apply binary deltas, or guarantee that a game will run in an emulator or port.

### Can I extract or export to an exFAT USB drive?

Yes. Orbis falls back to copying when the filesystem does not support exclusive
rename publication. Allow for temporary space and check `export --dry-run` first.
Use `--resume` from the start if you want to retain completed files after an
interruption. The [CLI guide](orbis-pkg-util/README.md#extract-all-or-selected-files)
explains partial files, conflicts, and safe retries.

### Does verification prove a package is authentic?

Verification detects unreadable or changed data using the available stored
hashes and HMACs. It does not verify publisher RSA signatures, licenses, or
publisher authenticity. Export manifests are unsigned records of copied files.

## Rust libraries

| Crate | Purpose |
| --- | --- |
| [orbis-pkg](orbis-pkg/) | Parse PS4 PKG archives, read metadata, and access package payloads |
| [orbis-pfs](orbis-pfs/) | Read PFS filesystem images and compressed file data |
| [orbis-pkg-util](orbis-pkg-util/) | CLI and library for extraction, update installation, and merged exports |
| [orbis-integrity](orbis-integrity/) | Shared streaming integrity-check types with no runtime dependencies |

## Build and test

```sh
git clone https://github.com/Antti/orbis.git
cd orbis
cargo build --release --locked
cargo test --workspace --all-features --locked
```

The executable is `target/release/orbis-pkg-util` (`orbis-pkg-util.exe` on Windows).
To install from a local checkout, run `cargo install --path orbis-pkg-util --locked`.
See [TESTING.md](TESTING.md) for regression coverage and real-filesystem tests,
and [RELEASE.md](RELEASE.md) for the version-bump release process.

## Performance

The original project reported the following timings for a roughly 30 GB
Bloodborne PKG (`CUSA03173_01`) on an Intel W-3175X, Samsung 990 Pro NVMe, and
192 GB DDR4-4000 six-channel memory:

| Tool | Min | Average | Max |
| --- | --- | --- | --- |
| `orbis-pkg-util` | 9 s | 16 s | 23 s |
| `shadPKG` | 73 s | 73 s | 75 s |

These are historical upstream results; the current release has not been benchmarked
under these conditions.
Storage speed, package contents, integrity checking, and filesystem copy fallbacks
affect runtime.

## Acknowledgements

- [videobitva/orbis](https://github.com/videobitva/orbis) — the original Rust implementation on which this fork is based.
- [shadPS4](https://github.com/shadps4-emu/shadPS4) — motivated the original project. Its since-removed PKG install code served as an initial format reference.
- [Obliteration](https://github.com/obhq/obliteration) — provided a Rust reference implementation for working with these formats.
- [shadPKG](https://github.com/seregonwar/ShadPKG) — used by the original project to validate extraction output.
- [PSDevWiki](https://www.psdevwiki.com/) — community-reverse-engineered PKG and PFS specifications.

## License

Licensed under either [Apache License 2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT), at your option.
