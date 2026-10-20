use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "orbis-pkg-util", version, propagate_version = true)]
#[command(about = "Inspect, extract, and check PS4 PKG files")]
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
    /// Display package metadata without requiring decryption keys
    #[command(
        after_help = "Examples:\n  orbis-pkg-util info game.pkg\n  orbis-pkg-util info game.pkg --json"
    )]
    Info {
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
        after_help = "Examples:\n  orbis-pkg-util verify game.pkg\n  orbis-pkg-util verify game.pkg --metadata-only\n  orbis-pkg-util verify game.pkg --path textures\n\nChecks selected entry payloads, filesystem structure, file lengths, and\ndecompression. Does not verify signatures, MACs, licenses, or authenticity.\nNo extracted files are written. Exit 3 means some selected data could not be checked."
    )]
    Verify {
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
