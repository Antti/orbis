//! Library for extracting PlayStation 4 PKG files.
//!
//! This crate provides the core extraction logic for PS4 `.pkg` files,
//! including both PKG entry extraction and inner PFS filesystem extraction.
//!
//! # Overview
//!
//! The main entry point is [`PkgExtractor`], which takes a parsed [`orbis_pkg::Pkg`]
//! and an [`ExtractProgress`] implementation, then extracts the package contents
//! to a directory on disk.
//!
//! # Example
//!
//! ```no_run
//! use orbis_cli::{open_pkg, PkgExtractor, SilentProgress};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let pkg = unsafe { open_pkg("game.pkg".as_ref()) }.expect("failed to open PKG");
//! let extractor = PkgExtractor::new(&pkg, SilentProgress, false);
//! extractor.extract("output/").expect("extraction failed");
//! # Ok(())
//! # }
//! ```
//!
//! # Custom progress reporting
//!
//! You can implement the [`ExtractProgress`] trait to receive fine-grained progress
//! updates during extraction.

pub mod extract;
mod fs_output;
pub mod install;
mod inventory;
pub mod progress;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_support;

pub use self::extract::{
    ExtractError, ExtractOptions, ExtractionReport, IntegrityReport, ListedItem, PkgExtractor,
    SkippedEntry,
};
pub use self::progress::{ExtractProgress, SilentProgress};

#[cfg(feature = "cli")]
pub use self::progress::ConsoleProgress;

use snafu::{ResultExt, Snafu};
use std::path::Path;

/// Errors that can occur when opening a PKG file from disk.
#[derive(Debug, Snafu)]
pub enum OpenPkgError {
    #[snafu(display("expected a regular PKG file, but the input is a directory or special file"))]
    NotFile,

    #[snafu(display("cannot inspect input file"))]
    Metadata { source: std::io::Error },

    #[snafu(display("failed to open file"))]
    OpenFile { source: std::io::Error },

    #[snafu(display("failed to memory map file"))]
    MmapFile { source: std::io::Error },

    #[snafu(display("failed to parse PKG"))]
    ParsePkg { source: orbis_pkg::OpenError },
}

/// Opens a PKG file from disk using a memory-mapped read.
///
/// This is a convenience function that opens the file, maps it into memory,
/// and parses the PKG header. The returned [`Pkg`](orbis_pkg::Pkg) borrows
/// from the memory map and is ready for extraction.
///
/// # Safety
///
/// This function uses `unsafe` internally to create a memory map. The caller
/// must ensure the file is not modified or truncated while the returned `Pkg`
/// is in use.
pub unsafe fn open_pkg(path: &Path) -> Result<orbis_pkg::Pkg<memmap2::Mmap>, OpenPkgError> {
    let mut pkg = unsafe { inspect_pkg(path)? };
    pkg.load_entry_keys().context(ParsePkgSnafu)?;
    pkg.load_pfs_key().context(ParsePkgSnafu)?;
    Ok(pkg)
}

/// Opens a PKG for inspection without loading decryption keys.
///
/// # Safety
///
/// The input file must not be modified or truncated while the returned package
/// is in use, as with [`open_pkg`].
pub unsafe fn inspect_pkg(path: &Path) -> Result<orbis_pkg::Pkg<memmap2::Mmap>, OpenPkgError> {
    // Check before opening: opening a directory fails on Windows, and opening
    // a FIFO can block on Unix. Recheck the actual handle below as well.
    snafu::ensure!(
        std::fs::metadata(path).context(MetadataSnafu)?.is_file(),
        NotFileSnafu
    );
    let file = std::fs::File::open(path).context(OpenFileSnafu)?;
    snafu::ensure!(
        file.metadata().context(MetadataSnafu)?.is_file(),
        NotFileSnafu
    );
    let raw = unsafe { memmap2::Mmap::map(&file).context(MmapFileSnafu)? };
    orbis_pkg::Pkg::parse(raw).context(ParsePkgSnafu)
}
