//! Portable export inventories. These hashes detect changes, not authenticity.
use super::{
    Result, invalid,
    inventory::{FileRecord, hash_file},
    storage,
};
use crate::{ExtractError, ExtractProgress};
use cap_std::{ambient_authority, fs::Dir};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
};

pub(super) const MANIFEST: &str = ".orbis-export.json";

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ExportManifest {
    pub schema_version: u32,
    pub title_id: String,
    pub content_id: String,
    pub base_version: String,
    pub update_version: Option<String>,
    pub complete: bool,
    pub directories: Vec<String>,
    pub files: Vec<FileRecord>,
}

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
#[derive(Debug, Serialize)]
pub struct DirectoryReport {
    pub schema_version: u32,
    pub title_id: String,
    pub base_version: String,
    pub update_version: Option<String>,
    pub complete: bool,
    pub checked_files: usize,
    pub issues: Vec<DirectoryIssue>,
}

pub(super) fn read_manifest(dir: &Dir) -> Result<ExportManifest> {
    let manifest: ExportManifest = storage::read_json(dir, MANIFEST)?;
    if manifest.schema_version != 1 {
        return Err(invalid("unsupported export manifest version"));
    }
    let mut paths = BTreeMap::new();
    for (path, directory) in manifest
        .directories
        .iter()
        .map(|p| (p, true))
        .chain(manifest.files.iter().map(|f| (&f.path, false)))
    {
        storage::validate_path(path)?;
        if paths
            .insert(path.to_ascii_lowercase(), (path, directory))
            .is_some()
        {
            return Err(invalid("duplicate or case-colliding export manifest path"));
        }
    }
    for (path, _) in paths.values() {
        let mut ancestor = Path::new(path).parent();
        while let Some(parent) = ancestor.filter(|p| !p.as_os_str().is_empty()) {
            let name = parent
                .to_str()
                .ok_or_else(|| invalid("invalid manifest path"))?;
            if !matches!(paths.get(&name.to_ascii_lowercase()), Some((stored, true)) if stored.as_str() == name)
            {
                return Err(invalid(
                    "manifest omits a parent directory or uses a file as a directory",
                ));
            }
            ancestor = parent.parent();
        }
    }
    for file in &manifest.files {
        if file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(invalid("invalid SHA-256 in export manifest"));
        }
    }
    Ok(manifest)
}

// macOS creates AppleDouble metadata on exFAT. Ignore only regular sidecars with
// an AppleDouble header and an existing regular file/directory counterpart.
pub(super) fn apple_double(dir: &Dir, name: &str) -> Result<bool> {
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

/// Read-only verification against the export's manifest. A modified manifest is
/// not authenticated; retain a trusted copy externally when provenance matters.
pub fn verify_directory(path: &Path, progress: impl ExtractProgress) -> Result<DirectoryReport> {
    let dir = Dir::open_ambient_dir(path, ambient_authority())?;
    let manifest = read_manifest(&dir)?;
    verify_open_directory(&dir, &manifest, &progress)
}

pub(super) fn verify_open_directory(
    dir: &Dir,
    manifest: &ExportManifest,
    progress: &impl ExtractProgress,
) -> Result<DirectoryReport> {
    let mut report = DirectoryReport {
        schema_version: 1,
        title_id: manifest.title_id.clone(),
        base_version: manifest.base_version.clone(),
        update_version: manifest.update_version.clone(),
        complete: manifest.complete,
        checked_files: 0,
        issues: Vec::new(),
    };
    let expected: BTreeMap<_, _> = manifest
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();
    let directories: BTreeSet<_> = manifest.directories.iter().map(String::as_str).collect();
    let mut seen = BTreeSet::new();
    let mut pending = vec![String::new()];
    let mut buffer = vec![0u8; 1024 * 1024];
    progress.phase("Verifying exported files");
    let total = manifest
        .files
        .iter()
        .try_fold(0u64, |n, f| n.checked_add(f.bytes))
        .ok_or_else(|| invalid("manifest size overflows"))?;
    progress.bytes_total(total);
    while let Some(prefix) = pending.pop() {
        if progress.is_cancelled() {
            return Err(ExtractError::Cancelled.into());
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
            if path == MANIFEST || apple_double(&current, &name)? {
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
