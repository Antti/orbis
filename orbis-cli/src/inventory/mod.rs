//! File inventories, hashing, and manifest I/O independent of installation policy.
use crate::ExtractProgress;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
};

mod directory;
mod json;
pub use directory::{DirectoryIssue, DirectoryIssueKind};
pub(crate) use directory::{apple_double, verify_tree};
#[cfg(test)]
pub(crate) use json::MAX_JSON;
pub(crate) use json::{read_json, sync_dir, write_json};

type Result<T> = std::result::Result<T, InventoryError>;

#[derive(Debug, Snafu)]
pub(crate) enum InventoryError {
    #[snafu(display("{message}"))]
    Invalid { message: String },
    #[snafu(transparent)]
    Io { source: std::io::Error },
    #[snafu(transparent)]
    Json { source: serde_json::Error },
    #[snafu(display("manifest is too large"))]
    ManifestTooLarge,
    #[snafu(display("inventory cancelled"))]
    Cancelled,
}

fn invalid(message: impl Into<String>) -> InventoryError {
    InventoryError::Invalid {
        message: message.into(),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileRecord {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
pub(crate) fn hash_file(
    root: &Dir,
    path: &str,
    buffer: &mut [u8],
    progress: &impl ExtractProgress,
) -> Result<FileRecord> {
    // Check every component so in-tree symlinks are rejected as well as escapes.
    let mut prefix = std::path::PathBuf::new();
    for part in Path::new(path).components() {
        prefix.push(part);
        let meta = root.symlink_metadata(&prefix)?;
        if (prefix == Path::new(path) && !meta.is_file())
            || (prefix != Path::new(path) && !meta.is_dir())
        {
            return Err(invalid(format!(
                "expected regular file and directories: {path}"
            )));
        }
    }
    let mut file = root.open(path)?;
    let before = file.metadata()?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    loop {
        if progress.is_cancelled() {
            return Err(InventoryError::Cancelled);
        }
        let n = file.read(buffer)?;
        if n == 0 {
            break;
        }
        bytes = bytes
            .checked_add(n as u64)
            .ok_or_else(|| invalid("file size overflows"))?;
        if bytes > before.len() {
            return Err(invalid(format!("file grew during verification: {path}")));
        }
        hash.update(&buffer[..n]);
        progress.bytes_processed(n as u64);
    }
    if before.len() != bytes || before.modified()? != file.metadata()?.modified()? {
        return Err(invalid(format!("file changed during verification: {path}")));
    }
    Ok(FileRecord {
        path: path.into(),
        bytes,
        sha256: format!("{:x}", hash.finalize()),
    })
}

pub(crate) struct InventoryDiff {
    pub matches: bool,
    pub modified_files: Vec<String>,
}

/// Compare full record lists as well as paths so duplicates and reordered records
/// remain mismatches even when the per-path contents agree.
pub(crate) fn compare_records(expected: &[FileRecord], actual: &[FileRecord]) -> InventoryDiff {
    let matches = actual == expected;
    if matches {
        return InventoryDiff {
            matches,
            modified_files: Vec::new(),
        };
    }
    let expected: BTreeMap<_, _> = expected.iter().map(|f| (f.path.as_str(), f)).collect();
    let actual: BTreeMap<_, _> = actual.iter().map(|f| (f.path.as_str(), f)).collect();
    let paths: BTreeSet<_> = expected.keys().chain(actual.keys()).copied().collect();
    let modified_files = paths
        .into_iter()
        .filter(|path| expected.get(path) != actual.get(path))
        .map(str::to_owned)
        .collect();
    InventoryDiff {
        matches,
        modified_files,
    }
}
