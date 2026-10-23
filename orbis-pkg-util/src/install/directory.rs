//! Portable export inventories. These hashes detect changes, not authenticity.
use super::{Result, invalid, storage};
use crate::ExtractProgress;
use crate::inventory::FileRecord;
pub use crate::inventory::{DirectoryIssue, DirectoryIssueKind};
use cap_std::{ambient_authority, fs::Dir};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

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
    progress.phase("Verifying exported files");
    let verification = crate::inventory::verify_tree(
        dir,
        &manifest.files,
        &manifest.directories,
        progress,
        |current, name, path| Ok(path == MANIFEST || crate::inventory::apple_double(current, name)?),
    )?;
    Ok(DirectoryReport {
        schema_version: 1,
        title_id: manifest.title_id.clone(),
        base_version: manifest.base_version.clone(),
        update_version: manifest.update_version.clone(),
        complete: manifest.complete,
        checked_files: verification.checked_files,
        issues: verification.issues,
    })
}
