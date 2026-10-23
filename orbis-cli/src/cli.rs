use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// Recognize the compatibility executable, while downloaded AppImage names and
/// arbitrary executable renames still advertise the canonical command.
pub fn invoked_name() -> &'static str {
    let executable = std::env::args_os().next().map(PathBuf::from);
    if executable.as_deref().and_then(|path| path.file_stem()) == Some("orbis-pkg-util".as_ref()) {
        "orbis-pkg-util"
    } else {
        "orbis"
    }
}

#[derive(Debug, Parser)]
#[command(name = invoked_name(), bin_name = invoked_name(), version, propagate_version = true)]
#[command(about = "Inspect and extract PS4 packages, install updates, and export merged games")]
#[command(
    after_help = "Examples:\n  orbis info game.pkg\n  orbis list game.pkg --json\n  orbis extract game.pkg -o ./game\n  orbis verify game.pkg\n\nExit codes: 0 complete, 1 failed, 2 invalid arguments, 3 incomplete, 130 interrupted."
)]
pub struct Cli {
    /// Hide progress and success messages; warnings and errors remain visible
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,
    /// Include the full error chain in diagnostics
    #[arg(short, long, global = true)]
    pub verbose: bool,
    /// Read a known 32-character package passcode from a file (- reads stdin)
    #[arg(long, global = true, value_name = "FILE")]
    pub passcode_file: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Default, Args)]
pub struct Selection {
    /// Process PKG metadata only; do not open the embedded filesystem
    #[arg(long)]
    pub metadata_only: bool,
    /// Select a file or directory using the paths from list (repeatable; no globs)
    #[arg(long = "path", value_name = "PATH", action = clap::ArgAction::Append)]
    pub paths: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Export the base and its installed update as one merged game directory
    #[command(
        after_help = "Example:\n  orbis export --base ./games/CUSA12345 -o ./bbport-game\n\nUpdate files take precedence; unchanged files come from the base. With no installed\nupdate, exports the base alone. Uses the managed sibling -UPDATE directory and\nvalidates its inventory. Installer metadata and rollback copies are excluded.\nThe output must be outside the sources, with an existing parent. By default it\nmust be new; --resume accepts a matching export manifest, verifies completed\nfiles, and copies missing files. Resumable output remains visible while incomplete.\nUse --dry-run to inspect versions and space; verify-dir checks exported hashes.\nWithout --resume, filesystems without exclusive rename (such as macOS exFAT)\nneed a second full copy and space for two merged games. Do not use the output until\nexport succeeds; forced termination can leave a partial destination.\nKeep the emulator stopped and source files unchanged until export completes.\nThis copies files; it does not decrypt executables or check game-specific requirements."
    )]
    Export {
        /// Original extracted base game directory used by install
        #[arg(long, value_name = "DIR")]
        base: PathBuf,
        /// Merged directory (must be new unless resuming a matching export)
        #[arg(short, long, value_name = "DIR")]
        output: PathBuf,
        /// Keep incomplete output and verify completed files when restarting
        #[arg(long, conflicts_with = "dry_run")]
        resume: bool,
        /// Inspect versions, file counts, filesystem limits, and peak space without writing
        #[arg(long)]
        dry_run: bool,
        /// Emit the dry-run plan as JSON
        #[arg(long, requires = "dry_run")]
        json: bool,
    },
    /// Install a full-file update separately, or merge it into the game folder
    #[command(
        after_help = "Examples:\n  orbis install update.pkg --base ./games/CUSA12345 --dry-run\n  orbis install update.pkg --base ./games/CUSA12345\n\nStop the emulator first. By default, the base directory is preserved.\nWith --merge, update files replace base files; originals are kept for rollback.\nMerge requires an original base without a sibling update. Restore with rollback\nbefore applying another update. Keep the adjacent .<game>.orbis-install folder;\nit contains the originals. Merge requires exclusive rename support (macOS exFAT\nis not supported); use separate updates or export on other filesystems. Only full-file cumulative\nor first updates with matching title/content IDs and a newer APP_VER are accepted.\nBinary deltas, downgrades, modified managed updates, and unmanaged update directories\nare refused. One previous installation is kept for rollback.\nAfter a process crash, run recover --base <DIR> before retrying."
    )]
    Install {
        #[arg(value_name = "PKG_FILE")]
        pkg_path: PathBuf,
        /// Original extracted base game directory
        #[arg(long, value_name = "DIR")]
        base: PathBuf,
        /// Show the version, path changes, and space estimate without writing anything
        #[arg(long)]
        dry_run: bool,
        /// Merge into the game folder, keeping replaced originals for rollback
        #[arg(long)]
        merge: bool,
        /// Emit the dry-run plan as schema-versioned JSON
        #[arg(long, requires = "dry_run")]
        json: bool,
    },
    /// Inspect installed versions, rollback, local changes, and recovery needs (no writes)
    #[command(alias = "doctor")]
    Status {
        #[arg(long, value_name = "DIR")]
        base: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Restore the previous managed update, or return to the base game
    #[command(
        after_help = "Example:\n  orbis rollback --base ./games/CUSA12345\n\nStop the emulator first. One rollback is available after each successful install.\nLocally modified update files are preserved by refusing rollback."
    )]
    Rollback {
        #[arg(long, value_name = "DIR")]
        base: PathBuf,
    },
    /// Recover an interrupted installation or rollback; clean abandoned staging files
    #[command(
        after_help = "Example:\n  orbis recover --base ./games/CUSA12345\n\nStop the emulator first. A pending transaction restores the previous installation.\nA committed transaction remains installed. Recovery is safe to repeat."
    )]
    Recover {
        #[arg(long, value_name = "DIR")]
        base: PathBuf,
    },
    /// Extract metadata and filesystem contents to a directory
    #[command(
        after_help = "Examples:\n  orbis extract game.pkg\n  orbis extract game.pkg -o ./game --path sce_sys/param.sfo --metadata-only\n  orbis extract game.pkg -o ./game --path textures\n\nThe default destination is ./<TITLE_ID> in the current directory. Existing files\nare preserved unless --force is supplied. --resume compares existing files with\nthe package and copies missing files; mismatches are preserved and reported.\nEach file is published after a complete\nwrite; files completed before a later failure remain. Exit 3 means entries were\nskipped. --quiet never hides those warnings."
    )]
    Extract {
        #[arg(value_name = "PKG_FILE")]
        pkg_path: PathBuf,
        /// Output directory (default: ./<TITLE_ID>)
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
        /// Replace existing files after successfully writing their replacements
        #[arg(short, long)]
        force: bool,
        /// Verify and reuse completed files; refuse mismatches instead of overwriting
        #[arg(long, conflicts_with = "force")]
        resume: bool,
        #[command(flatten)]
        selection: Selection,
    },
    /// Display header and PARAM.SFO metadata
    #[command(
        after_help = "Examples:\n  orbis info game.pkg\n  orbis info game.pkg --json\n  orbis info game.pkg --sfo\n  orbis info game.pkg --header-only\n\nUnreadable or malformed PARAM.SFO leaves header output available and returns exit 3."
    )]
    Info {
        /// Read only the header, without opening PARAM.SFO or loading keys
        #[arg(long, conflicts_with = "sfo")]
        header_only: bool,
        /// Show every typed PARAM.SFO field (JSON always includes all fields)
        #[arg(long)]
        sfo: bool,
        #[arg(value_name = "PKG_FILE")]
        pkg_path: PathBuf,
        /// Emit schema-versioned JSON; byte counts remain numeric
        #[arg(long)]
        json: bool,
    },
    /// List metadata and filesystem contents using extraction-relative paths
    #[command(
        after_help = "Examples:\n  orbis list game.pkg\n  orbis list game.pkg --entries\n  orbis list game.pkg --files --json\n\nBy default both scopes are listed. If the filesystem cannot be opened, metadata\nis still shown with a warning and exit code 3. --entries needs no decryption keys."
    )]
    List {
        #[arg(value_name = "PKG_FILE")]
        pkg_path: PathBuf,
        /// List only the PKG entry table, without opening the filesystem
        #[arg(long, conflicts_with = "files")]
        entries: bool,
        /// List only filesystem files and directories
        #[arg(long, conflicts_with = "entries")]
        files: bool,
        /// Emit schema-versioned JSON with items and warnings
        #[arg(long)]
        json: bool,
    },
    /// Check readability and decompression without extracting files
    #[command(
        after_help = "Examples:\n  orbis verify game.pkg\n  orbis verify game.pkg --metadata-only\n  orbis verify game.pkg --path textures\n  orbis verify game.pkg --integrity --json\n\nChecks selected entry payloads, filesystem structure, lengths, and decompression.\nAdd --integrity to verify stored PKG SHA-256 digests and signed PFS block HMACs.\nIt cannot be combined with --path; --metadata-only limits checks to PKG metadata.\nAbsent hashes or unavailable MACs return exit 3, even when readability passes.\nPublisher signatures, licenses, and authenticity are not verified. No files are written."
    )]
    Verify {
        /// Also check stored PKG digests and signed PFS block HMACs
        #[arg(long, conflicts_with = "paths")]
        integrity: bool,
        /// Emit a streamed JSON integrity report, including failures and omissions
        #[arg(long, requires = "integrity")]
        json: bool,
        #[arg(value_name = "PKG_FILE")]
        pkg_path: PathBuf,
        #[command(flatten)]
        selection: Selection,
    },
    /// Check an exported directory against its SHA-256 manifest (no writes)
    VerifyDir {
        #[arg(value_name = "DIR")]
        directory: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Generate a shell completion script on stdout
    #[command(
        after_help = "Examples:\n  orbis completions bash > ~/.local/share/bash-completion/completions/orbis\n  orbis completions zsh > ~/.zfunc/_orbis\n  orbis completions fish > ~/.config/fish/completions/orbis.fish\n\nCreate the destination directory first. For zsh, add ~/.zfunc to fpath before compinit."
    )]
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}
