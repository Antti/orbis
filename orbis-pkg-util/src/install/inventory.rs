//! Shared file records and stable, cancellable hashing for managed trees.
use super::{Result, invalid};
use crate::{ExtractError, ExtractProgress};
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FileRecord {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
pub(super) fn hash_file(
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
            return Err(ExtractError::Cancelled.into());
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
