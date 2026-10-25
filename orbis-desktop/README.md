# Orbis desktop

A desktop app for opening and verifying PlayStation 4 PKGs, extracting files,
installing supported full-file updates, and exporting merged games. Updates can
stay separate for shadPS4 or be merged into a game folder. It calls the same Rust libraries
as the CLI; no command-line executable or subprocess is needed for package work.

See the [desktop screenshots](../README.md#desktop-screenshots) for package inspection and update setup.

## Open the app

Use an **orbis-desktop** archive from v0.4.0 or later.
Extract the archive, then open `Orbis.exe` on Windows, `Orbis.app` on macOS, or
`Orbis` on Linux. On macOS, move the app to Applications first.
The separate **orbis-cli** downloads provide the `orbis` command-line tool.
Keep the desktop and CLI in separate folders on Windows, where `Orbis.exe` and
`orbis.exe` are the same filename.
Older `orbis-pkg-util` downloads are also command-line tools.

Click **Open PKG**, or drop one PKG onto the window. The compact summary shows
artwork, title, title ID, package type, application version, PKG size, and minimum
system version when available. **Package details** expands the content ID,
package version, region inferred from the ID, and source path. These are package
metadata, not a compatibility or integrity check. Missing metadata keys are
flagged before extraction. macOS installer `.pkg` files cannot be read by Orbis.

- **Extract files:** choose a destination and a new folder name, then extract.
  Existing files are never replaced. To continue interrupted work, select the
  same destination and folder name and enable **Resume**. Matching files are
  checked before reuse; conflicting files remain untouched.
- **Install update:** open a supported update PKG, choose the extracted base game
  folder containing `sce_sys`, and click **Check update**. Review the versions,
  destination, file changes, and space estimate. **Keep update separate** is the
  default: **Install update** writes to the sibling `-UPDATE` folder and preserves
  the base. **Merge into game folder** writes update files directly into the base;
  review the preview, close the emulator, then click **Merge update**. Replaced
  originals are moved to a backup beside the game; unchanged files stay in place.
- **Manage game:** choose a game folder to find a merged update. **Restore base**
  restores replaced originals and removes update-only files. It works without the
  PKG. An interrupted merge offers **Recover base**. Both check files before
  restoring; local changes must be resolved first.
- **Cancel:** stops cooperatively between work chunks. Completed extraction files
  remain. Closing during work offers to cancel and waits for cleanup before exit.
- **Show in folder:** opens the output directory after extraction or installation.

## Verification, passcodes, and export (v0.5.0)

**Verify package** offers full integrity and readability checks or quicker
metadata-only checks. Full verification checks stored SHA-256 values, supported
PFS HMACs, and file readability/decompression. Metadata-only excludes game data.
Results distinguish passed, incomplete, failed, and cancelled operations; missing
hashes or skipped entries are incomplete, not passed. Verification reads the PKG
without extracting it and supports Cancel.

**Save verification report** saves a JSON summary with counts, unavailable-check
reasons, skipped entries, scope, time, and any error. It uses
`report_type: "orbis-desktop-verification-summary"` and `schema_version: 1`.
Individual block checks are not retained; use the CLI for the streamed full
check report. Choose a new filename: saving never replaces an existing file.
Stored-data consistency does not establish publisher authenticity.

Expand **Package passcode** to enter a known 32-character printable ASCII
passcode, then select **Validate passcode**. All seven entry-key digests must
match before the passcode is accepted. Package details refresh to show unlocked
metadata; extraction, update installation, and verification use the validated
passcode. A rejected replacement keeps the previous validated passcode. The
value remains only in memory for the open package, is excluded from reports,
and is cleared when another package opens or **Clear passcode** is selected.
Validation and clearing also discard the input field's undo history.
This does not decrypt arbitrary retail filesystems.

**Export game…** works without opening a PKG. Choose the original extracted base,
the destination parent, and an output folder name. Orbis includes its managed
separate update automatically; with none installed, it exports the base alone.
Restore an in-place merge before exporting. **Check export** previews versions,
file counts, destination, and a conservative free-space reserve.

Resume is enabled by default. It checks source hashes, the existing export
manifest, and completed files before reusing them; conflicting files remain
untouched. The preview estimates space for missing files, including filesystem
overhead. **Export and verify** rechecks the preview, copies the files, and then
verifies the copied directory against the manifest produced by that export.
Replacing the destination or rewriting its manifest makes verification fail.
Keep the emulator stopped and source files unchanged. A cancelled resumable copy retains completed files;
repeat the same selection to continue. If verification is cancelled after the
copy finishes, the export remains available, but should be resumed and verified
before use. Original game files and the installed update remain unchanged.

Keep the input PKG unchanged while Orbis is working. Missing decryption keys or
skipped entries produce an **incomplete** result, even if other files extracted
successfully. Binary delta updates and mismatched/downgrade updates are refused.
Merge accepts an original base with no separate update. Restore it before
applying another update; this avoids guessing what files omitted from a later
patch mean. Keep the adjacent `.<game>.orbis-install` directory with your game:
it contains the originals needed for restoration. Merge requires filesystem
support for exclusive renames; macOS exFAT users should use separate updates and
**Export game…** instead. The update preview estimates patch staging space and reports the
size of retained originals; it does not reserve space.

Separate updates retain one previous installation. Rollback and recovery for
separate updates, selective extraction, and streamed per-check verification
reports remain in the [CLI](../orbis-cli/README.md).

The app accepts one package path as its first argument. Windows **Open with** can
use `Orbis.exe`; Linux launchers can pass a file path to `Orbis`. The app does not change
default file associations. On macOS, use the app's picker or drag and drop;
Finder document-open events are not supported in this version.

## Build and run

Requires Rust 1.95 or later, a graphical session, and an OpenGL-capable driver.
Linux file dialogs use an XDG desktop portal backend (for example
`xdg-desktop-portal-gtk` or the desktop's KDE/GNOME backend). Linux builds also
need the development packages `libxkbcommon-dev` and `libwayland-dev` on Debian/Ubuntu.

```sh
cargo run -p orbis-desktop --locked
cargo run -p orbis-desktop --locked -- /path/to/game.pkg
cargo build -p orbis-desktop --release --locked
```

The GUI is a separate workspace member. Plain `cargo build` still builds only
the libraries and CLI; their dependencies and runtime requirements remain separate.
GUI builds use [eframe/egui](https://docs.rs/eframe/0.37.0/eframe/) and
[rfd native file dialogs](https://docs.rs/rfd/0.17.2/rfd/).
The bundled [Inter font](https://github.com/google/fonts/tree/main/ofl/inter)
uses the [SIL Open Font License](assets/fonts/OFL.txt); release apps include it.

Build a double-clickable macOS app archive locally:

The packaging script requires Python 3.11 or later.

```sh
# Use x86_64-apple-darwin on an Intel Mac.
python3 scripts/release/package-desktop.py 0.5.0 aarch64-apple-darwin \
  target/release/orbis-desktop dist
```

The packager also supports Windows x86-64 and Linux x86-64/ARM64. Linux desktop
archives use GNU libc (Ubuntu 22.04+ for x86-64, Ubuntu 24.04+ for ARM64), unlike
the static CLI packages. Release assets use the CLI's release version; the
desktop crate has its own development version. See [release automation](../RELEASE.md).

## Development checks

```sh
cargo test -p orbis-desktop --locked
cargo clippy -p orbis-desktop --all-targets --locked -- -D warnings
python3 -m unittest discover -s scripts/release -p 'test_*.py'
```

Synthetic packages exercise the real parser/extractor/installer without a
display. Before release, also launch on each supported platform and check native
dialogs, drag and drop, extraction, cancellation, close-during-work, update
preview/install, merge/restore/recovery, passcode validation/clearing, verification
and report saving, export/resume/verification, and opening the output folder. Use a real package and an
original base/update pair for compatibility testing.
