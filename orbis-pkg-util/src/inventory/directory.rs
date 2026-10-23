//! Verification of file records and expected directories within a capability.
use super::{FileRecord, InventoryError, Result, hash_file, invalid};
use crate::ExtractProgress;
use cap_std::fs::Dir;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

/// Stable categories used by verification reports and resume conflict checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryIssueKind {
    Missing,
    Changed,
    Unexpected,
    WrongType,
}

impl std::fmt::Display for DirectoryIssueKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Missing => "missing",
            Self::Changed => "changed",
            Self::Unexpected => "unexpected",
            Self::WrongType => "wrong_type",
        })
    }
}

#[derive(Debug, Serialize)]
pub struct DirectoryIssue {
    pub path: String,
    pub kind: DirectoryIssueKind,
}

pub(crate) struct TreeVerification {
    pub checked_files: usize,
    pub issues: Vec<DirectoryIssue>,
}

// macOS creates AppleDouble metadata on exFAT. Ignore only regular sidecars with
// an AppleDouble header and an existing regular file/directory counterpart.
pub(crate) fn apple_double(dir: &Dir, name: &str) -> Result<bool> {
    let Some(original) = name.strip_prefix("._").filter(|s| !s.is_empty()) else {
        return Ok(false);
    };
    if !dir.symlink_metadata(name)?.is_file() {
        return Ok(false);
    }
    let counterpart = match dir.symlink_metadata(original) {
        Ok(m) => m.is_file() || m.is_dir(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    let mut header = [0; 8];
    Ok(counterpart
        && dir.open(name)?.read_exact(&mut header).is_ok()
        && header == [0, 5, 22, 7, 0, 2, 0, 0])
}

/// Callers validate manifest paths and decide which entries to ignore. The skip
/// callback receives the containing directory, entry name, and root-relative path.
pub(crate) fn verify_tree(
    dir: &Dir,
    files: &[FileRecord],
    directories: &[String],
    progress: &impl ExtractProgress,
    skip: impl Fn(&Dir, &str, &str) -> Result<bool>,
) -> Result<TreeVerification> {
    let mut report = TreeVerification {
        checked_files: 0,
        issues: Vec::new(),
    };
    let expected: BTreeMap<_, _> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    let directories: BTreeSet<_> = directories.iter().map(String::as_str).collect();
    let mut seen = BTreeSet::new();
    let mut pending = vec![String::new()];
    let mut buffer = vec![0u8; 1024 * 1024];
    let total = files
        .iter()
        .try_fold(0u64, |n, f| n.checked_add(f.bytes))
        .ok_or_else(|| invalid("manifest size overflows"))?;
    progress.bytes_total(total);
    while let Some(prefix) = pending.pop() {
        if progress.is_cancelled() {
            return Err(InventoryError::Cancelled);
        }
        let current = dir.open_dir(if prefix.is_empty() { "." } else { &prefix })?;
        for entry in current.entries()? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if skip(&current, &name, &path)? {
                continue;
            }
            seen.insert(path.clone());
            let metadata = current.symlink_metadata(&name)?;
            let kind = if let Some(expected) = expected.get(path.as_str()) {
                if !metadata.is_file() {
                    Some(DirectoryIssueKind::WrongType)
                } else {
                    report.checked_files += 1;
                    if &hash_file(dir, &path, &mut buffer, progress)? != *expected {
                        Some(DirectoryIssueKind::Changed)
                    } else {
                        None
                    }
                }
            } else if directories.contains(path.as_str()) {
                if metadata.is_dir() {
                    pending.push(path.clone());
                    None
                } else {
                    Some(DirectoryIssueKind::WrongType)
                }
            } else {
                Some(DirectoryIssueKind::Unexpected)
            };
            if let Some(kind) = kind {
                report.issues.push(DirectoryIssue { path, kind });
            }
        }
    }
    for path in expected.keys().copied().chain(directories.iter().copied()) {
        if !seen.contains(path) {
            report.issues.push(DirectoryIssue {
                path: path.into(),
                kind: DirectoryIssueKind::Missing,
            });
        }
    }
    report.issues.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(report)
}
