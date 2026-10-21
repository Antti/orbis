use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "orbis-pkg-util", version, propagate_version = true)]
#[command(about = "Inspect, extract, check, and install PS4 PKG updates")]
#[command(
    after_help = "Examples:\n  orbis-pkg-util info game.pkg\n  orbis-pkg-util list game.pkg --json\n  orbis-pkg-util extract game.pkg -o ./game\n  orbis-pkg-util verify game.pkg\n\nExit codes: 0 complete, 1 failed, 2 invalid arguments, 3 incomplete, 130 interrupted."
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
    /// Install a full-file update into the base directory's sibling -UPDATE directory
    #[command(
        after_help = "Examples:\n  orbis-pkg-util install update.pkg --base ./games/CUSA12345 --dry-run\n  orbis-pkg-util install update.pkg --base ./games/CUSA12345\n\nStop the emulator first. The base directory is preserved. Only full-file cumulative\nor first updates with matching title/content IDs and a newer APP_VER are accepted.\nBinary deltas, downgrades, modified managed updates, and unmanaged update directories\nare refused. One previous installation is kept for rollback.\nAfter a process crash, run recover --base <DIR> before retrying."
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
        /// Emit the dry-run plan as schema-versioned JSON
        #[arg(long, requires = "dry_run")]
        json: bool,
    },
    /// Restore the previous managed update, or return to the base game
    #[command(
        after_help = "Example:\n  orbis-pkg-util rollback --base ./games/CUSA12345\n\nStop the emulator first. One rollback is available after each successful install.\nLocally modified update files are preserved by refusing rollback."
    )]
    Rollback {
        #[arg(long, value_name = "DIR")]
        base: PathBuf,
    },
    /// Recover an interrupted installation or rollback; clean abandoned staging files
    #[command(
        after_help = "Example:\n  orbis-pkg-util recover --base ./games/CUSA12345\n\nStop the emulator first. A pending transaction restores the previous installation.\nA committed transaction remains installed. Recovery is safe to repeat."
    )]
    Recover {
        #[arg(long, value_name = "DIR")]
        base: PathBuf,
    },
    /// Extract metadata and filesystem contents to a directory
    #[command(
        after_help = "Examples:\n  orbis-pkg-util extract game.pkg\n  orbis-pkg-util extract game.pkg -o ./game --path sce_sys/param.sfo --metadata-only\n  orbis-pkg-util extract game.pkg -o ./game --path textures\n\nThe default destination is ./<TITLE_ID> in the current directory. Existing files\nare preserved unless --force is supplied. Each file is published after a complete\nwrite; files completed before a later failure remain. Exit 3 means entries were\nskipped. --quiet never hides those warnings."
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
        #[command(flatten)]
        selection: Selection,
    },
    /// Display header and PARAM.SFO metadata
    #[command(
        after_help = "Examples:\n  orbis-pkg-util info game.pkg\n  orbis-pkg-util info game.pkg --json\n  orbis-pkg-util info game.pkg --sfo\n  orbis-pkg-util info game.pkg --header-only\n\nUnreadable or malformed PARAM.SFO leaves header output available and returns exit 3."
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
        after_help = "Examples:\n  orbis-pkg-util list game.pkg\n  orbis-pkg-util list game.pkg --entries\n  orbis-pkg-util list game.pkg --files --json\n\nBy default both scopes are listed. If the filesystem cannot be opened, metadata\nis still shown with a warning and exit code 3. --entries needs no decryption keys."
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
        after_help = "Examples:\n  orbis-pkg-util verify game.pkg\n  orbis-pkg-util verify game.pkg --metadata-only\n  orbis-pkg-util verify game.pkg --path textures\n  orbis-pkg-util verify game.pkg --integrity --json\n\nChecks selected entry payloads, filesystem structure, lengths, and decompression.\nAdd --integrity to verify stored PKG SHA-256 digests and signed PFS block HMACs.\nIt cannot be combined with --path; --metadata-only limits checks to PKG metadata.\nAbsent hashes or unavailable MACs return exit 3, even when readability passes.\nPublisher signatures, licenses, and authenticity are not verified. No files are written."
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
    /// Generate a shell completion script on stdout
    #[command(
        after_help = "Examples:\n  orbis-pkg-util completions bash > ~/.local/share/bash-completion/completions/orbis-pkg-util\n  orbis-pkg-util completions zsh > ~/.zfunc/_orbis-pkg-util\n  orbis-pkg-util completions fish > ~/.config/fish/completions/orbis-pkg-util.fish\n\nCreate the destination directory first. For zsh, add ~/.zfunc to fpath before compinit."
    )]
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}
