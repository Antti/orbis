//! Full-file update installation into shadPS4's sibling `-UPDATE` directory.
//!
//! The base is never modified. Activation uses a recoverable sequence of directory
//! renames, not a cross-platform atomic directory exchange. Stop the emulator first.
mod storage;
mod transaction;

use crate::{ExtractError, ExtractOptions, ExtractProgress, PkgExtractor, SilentProgress};
use cap_std::fs::Dir;
use orbis_pkg::{
    Pkg,
    entry::EntryId,
    header::ContentFlags,
    sfo::{AppMetadata, AppVersion},
};
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use storage::{Layout, Manifest, inventory, read_json};

pub(crate) type Result<T> = std::result::Result<T, InstallError>;

#[derive(Debug, Snafu)]
pub enum InstallError {
    #[snafu(display("{message}"))]
    Invalid { message: String },
    #[snafu(display("installation filesystem operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("invalid installation metadata"))]
    Metadata { source: orbis_pkg::sfo::SfoError },
    #[snafu(display("update extraction failed"))]
    Extract { source: ExtractError },
    #[snafu(display("invalid installation state"))]
    State { source: serde_json::Error },
}
impl From<std::io::Error> for InstallError {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}
impl From<ExtractError> for InstallError {
    fn from(source: ExtractError) -> Self {
        Self::Extract { source }
    }
}
impl From<orbis_pkg::sfo::SfoError> for InstallError {
    fn from(source: orbis_pkg::sfo::SfoError) -> Self {
        Self::Metadata { source }
    }
}
impl From<serde_json::Error> for InstallError {
    fn from(source: serde_json::Error) -> Self {
        Self::State { source }
    }
}
pub(crate) fn invalid(message: impl Into<String>) -> InstallError {
    InstallError::Invalid {
        message: message.into(),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Add,
    Replace,
    RemoveFromUpdate,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileChange {
    pub path: String,
    pub change: ChangeKind,
}

/// A read-only plan. Sizes are estimates; extraction can still run out of space.
#[derive(Debug, Serialize)]
pub struct InstallPlan {
    pub title_id: String,
    pub content_id: String,
    pub current_version: String,
    pub update_version: String,
    #[serde(serialize_with = "serialize_path")]
    pub destination: PathBuf,
    pub extracted_bytes: u64,
    pub required_free_bytes: u64,
    pub available_bytes: u64,
    pub files: Vec<FileChange>,
}

fn serialize_path<S: serde::Serializer>(
    path: &Path,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

/// Inspects compatibility and affected paths without creating directories or locks.
/// `pkg` must have the keys required to list its filesystem.
pub fn plan<R: AsRef<[u8]> + Sync>(pkg: &Pkg<R>, base: &Path) -> Result<InstallPlan> {
    plan_with_progress(pkg, base, SilentProgress)
}

/// Plans an update with progress and cooperative cancellation during inventory reads.
pub fn plan_with_progress<R: AsRef<[u8]> + Sync, P: ExtractProgress>(
    pkg: &Pkg<R>,
    base: &Path,
    progress: P,
) -> Result<InstallPlan> {
    let location = Layout::open(base, false)?;
    make_plan(pkg, &location, &progress)
}

/// Validates the supported update format and identity before opening the filesystem.
pub fn update_metadata<R: AsRef<[u8]>>(pkg: &Pkg<R>) -> Result<AppMetadata> {
    let header = pkg.header();
    let flags = header.content_flags();
    if header.content_type() == 0x1e || flags.intersects(ContentFlags::DELTA_PATCH_X) {
        return Err(invalid(
            "binary delta packages are not supported; provide a full-file update",
        ));
    }
    if header.content_type() != 0x1a
        || !(flags.contains(ContentFlags::CUMULATIVE_PATCH)
            || (flags.contains(ContentFlags::FIRST_PATCH)
                && !flags.contains(ContentFlags::SUBSEQUENT_PATCH)))
    {
        return Err(invalid(
            "expected a PS4 full-file cumulative or first update package",
        ));
    }
    let mut sfo = None;
    for entry in pkg.entries() {
        let (_, entry) = entry.map_err(|e| invalid(e.to_string()))?;
        if entry.entry_id() == EntryId::ParamSfo {
            if sfo.is_some() || entry.data_size() > 1024 * 1024 {
                return Err(invalid("duplicate or oversized PARAM.SFO entry"));
            }
            sfo = Some(entry);
        }
    }
    let entry = sfo.ok_or_else(|| invalid("update has no PARAM.SFO"))?;
    let data = pkg.entry_data(&entry).map_err(|e| invalid(e.to_string()))?;
    let metadata = AppMetadata::parse(&data)?;
    if metadata.category != "gp" {
        return Err(invalid("update PARAM.SFO CATEGORY must be gp"));
    }
    if metadata.content_id != header.content_id().as_str() {
        return Err(invalid("PKG header and PARAM.SFO content IDs disagree"));
    }
    Ok(metadata)
}

fn make_plan<R: AsRef<[u8]> + Sync, P: ExtractProgress>(
    pkg: &Pkg<R>,
    location: &Layout,
    progress: &P,
) -> Result<InstallPlan> {
    if progress.is_cancelled() {
        return Err(ExtractError::Cancelled.into());
    }
    location.require_recovered()?;
    let base = location.base_metadata()?;
    let update = update_metadata(pkg)?;
    if update.content_id != base.content_id || update.title_id != base.title_id {
        return Err(invalid(
            "update does not match the base game's title and content ID",
        ));
    }
    let state = location.state()?;
    let current = location.active_manifest(&state, &base, progress)?;
    location.verify_previous(&state, &base, progress)?;
    let current_version = current
        .as_ref()
        .map_or(Ok(base.version), |m| m.app_version.parse::<AppVersion>())?;
    if update.version <= current_version {
        return Err(invalid(format!(
            "update {} is not newer than installed {}; repeated installs and downgrades are refused",
            update.version, current_version
        )));
    }
    let extractor = PkgExtractor::new(pkg, SilentProgress, false);
    let entries = extractor.list_entries()?;
    if entries.iter().any(|e| e.path.is_none()) {
        return Err(invalid(
            "update contains unknown metadata entries; installation requires complete extraction",
        ));
    }
    let mut items = entries;
    items.extend(extractor.list_files()?);
    let mut paths = BTreeSet::new();
    let mut normalized = BTreeSet::new();
    let mut bytes = 0u64;
    let mut changes = Vec::new();
    let active_paths: BTreeSet<_> = current
        .iter()
        .flat_map(|m| m.files.iter().map(|f| f.path.as_str()))
        .collect();
    let base_dir = location.base_dir()?;
    for item in items {
        if progress.is_cancelled() {
            return Err(ExtractError::Cancelled.into());
        }
        let path = item.path.unwrap();
        storage::validate_path(&path)?;
        match base_dir.symlink_metadata(&path) {
            Ok(meta)
                if meta.file_type().is_symlink()
                    || (item.is_directory && !meta.is_dir())
                    || (!item.is_directory && !meta.is_file()) =>
            {
                return Err(invalid(format!("base file/directory conflict: {path}")));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        if !normalized.insert(path.to_ascii_lowercase()) {
            return Err(invalid(format!("duplicate or case-colliding path: {path}")));
        }
        // A file in an ancestor position must be caught before extraction starts.
        if !item.is_directory {
            paths.insert(path.clone());
            bytes = bytes
                .checked_add(item.size)
                .ok_or_else(|| invalid("update size overflows"))?;
            let change = if active_paths.contains(path.as_str()) || base_dir.try_exists(&path)? {
                ChangeKind::Replace
            } else {
                ChangeKind::Add
            };
            changes.push(FileChange { path, change });
        }
    }
    let folded_paths: BTreeSet<_> = paths.iter().map(|p| p.to_ascii_lowercase()).collect();
    for path in &paths {
        let mut parent = Path::new(path).parent();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            if folded_paths.contains(&p.to_string_lossy().to_ascii_lowercase()) {
                return Err(invalid(format!("file/directory conflict: {path}")));
            }
            parent = p.parent();
        }
    }
    for path in active_paths {
        if !paths.contains(path) {
            changes.push(FileChange {
                path: path.into(),
                change: ChangeKind::RemoveFromUpdate,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    let required = bytes
        .checked_add(1024 * 1024)
        .and_then(|n| n.checked_add(paths.len() as u64 * 256))
        .ok_or_else(|| invalid("update size overflows"))?;
    Ok(InstallPlan {
        title_id: update.title_id,
        content_id: update.content_id,
        current_version: current_version.to_string(),
        update_version: update.version.to_string(),
        destination: location.active_path(),
        extracted_bytes: bytes,
        required_free_bytes: required,
        available_bytes: fs2::available_space(&location.parent_path)?,
        files: changes,
    })
}

fn check_space(required: u64, available: u64) -> Result<()> {
    if available < required {
        return Err(invalid(format!(
            "insufficient free space: need approximately {required} bytes, available {available}"
        )));
    }
    Ok(())
}

/// Installs a complete update. A failed extraction never changes the active update.
/// Keeps one previous update (or the base-only state) available for [`rollback`].
pub fn install<R: AsRef<[u8]> + Sync, P: ExtractProgress>(
    pkg: &Pkg<R>,
    base: &Path,
    progress: P,
) -> Result<InstallPlan> {
    let location = Layout::open(base, true)?;
    location.require_recovered()?;
    let plan = make_plan(pkg, &location, &progress)?;
    check_space(plan.required_free_bytes, plan.available_bytes)?;
    location.clean_uncommitted()?;
    let manager = location.manager()?;
    manager.create_dir("staging")?;
    let result = (|| {
        let report = PkgExtractor::new(pkg, &progress, false)
            .extract_with_options(location.stage_path(), &ExtractOptions::default())?;
        if !report.skipped.is_empty() {
            return Err(invalid(
                "update extraction was incomplete; active update was preserved",
            ));
        }
        progress.phase("Checking staged update");
        progress.bytes_total(plan.extracted_bytes);
        let stage = manager.open_dir("staging")?;
        let actual = storage::app_metadata(&stage)?;
        if actual.content_id != plan.content_id
            || actual.version.to_string() != plan.update_version
            || actual.category != "gp"
        {
            return Err(invalid(
                "staged PARAM.SFO does not match the installation plan",
            ));
        }
        let files = inventory(&stage, &progress)?;
        let expected: BTreeSet<_> = plan
            .files
            .iter()
            .filter(|f| f.change != ChangeKind::RemoveFromUpdate)
            .map(|f| f.path.as_str())
            .collect();
        if files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<BTreeSet<_>>()
            != expected
        {
            return Err(invalid(
                "staged file paths differ from the plan; possible filesystem name collision",
            ));
        }
        let manifest = Manifest {
            schema_version: 1,
            title_id: plan.title_id.clone(),
            content_id: plan.content_id.clone(),
            app_version: plan.update_version.clone(),
            files,
        };
        storage::write_json(&stage, storage::MANIFEST, &manifest)?;
        storage::sync_dir(&stage)?;
        drop(stage);
        if progress.is_cancelled() {
            return Err(ExtractError::Cancelled.into());
        }
        // Do not interrupt the short activation sequence. Journal recovery is used
        // if the process exits or an I/O operation fails during these renames.
        progress.phase("Activating update");
        location.activate()
    })();
    if let Err(error) = result {
        location.restore_after_error(&error)?;
        return Err(error);
    }
    Ok(plan)
}

/// Restores the previous managed update, or removes the first update to use the base.
/// Only one rollback is retained. Modified/unmanaged trees are never overwritten.
pub fn rollback(base: &Path) -> Result<()> {
    rollback_with_progress(base, SilentProgress)
}

/// Rolls back with cancellable checks of the active and previous file inventories.
pub fn rollback_with_progress<P: ExtractProgress>(base: &Path, progress: P) -> Result<()> {
    let location = Layout::open(base, true)?;
    location.require_recovered()?;
    let state = location.state()?;
    let base_meta = location.base_metadata()?;
    location.active_manifest(&state, &base_meta, &progress)?;
    location.verify_previous(&state, &base_meta, &progress)?;
    match state.rollback {
        storage::RollbackTarget::None => {
            return Err(invalid(
                "no previous installation is available for rollback",
            ));
        }
        storage::RollbackTarget::Update | storage::RollbackTarget::Base => {}
    }
    if progress.is_cancelled() {
        return Err(ExtractError::Cancelled.into());
    }
    location.clean_uncommitted()?;
    if let Err(error) = location.rollback_transaction() {
        location.restore_after_error(&error)?;
        return Err(error);
    }
    Ok(())
}

/// Recovers an interrupted activation/rollback and removes abandoned staging data.
/// With no pending transaction this is a harmless cleanup operation.
pub fn recover(base: &Path) -> Result<()> {
    let location = Layout::open(base, true)?;
    location.recover_transaction()?;
    location.clean_uncommitted()
}

#[cfg(test)]
mod tests;
