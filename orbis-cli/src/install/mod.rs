//! Full-file update installation into shadPS4's sibling `-UPDATE` directory,
//! and independent merged game exports for consumers that need a single tree.
//!
//! Separate updates preserve the base. Merge mode journals per-file replacements
//! and retains original files for restoration. Stop the emulator first.
mod compatibility;
use compatibility::check_update_version;
pub use compatibility::{
    LockedUpdate, UpdateCheck, UpdateReadiness, check_update_identity, inspect_update,
    update_metadata,
};
mod directory;
mod export;
mod merge;
pub use directory::{DirectoryIssue, DirectoryIssueKind, DirectoryReport, verify_directory};
pub use merge::{MergeStatus, merge_status};
mod preflight;
mod rollback;
mod status;
mod storage;
mod transaction;
pub use rollback::{SeparateRollback, rollback, rollback_with_progress};
pub use status::{GameTarget, InstallationStatus, status, status_with_rollback};

pub use export::{
    ExportEstimate, ExportPlan, ExportPreview, ExportReport, ExportedDirectory, ResumePlan,
    export_for_verification, export_merged, plan_export, plan_export_with_resume, resume_export,
};
pub use preflight::FilesystemPlan;

use crate::{ExtractError, ExtractOptions, ExtractProgress, PkgExtractor, SilentProgress};
use cap_std::fs::Dir;
use orbis_pkg::{
    EntryDataError, ParamSfoError, Pkg,
    header::{ContentFlags, ContentId},
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
    #[snafu(display("update does not match the base game's title and content ID"))]
    UpdateIdentityMismatch,
    #[snafu(display("{message}"))]
    Invalid { message: String },
    #[snafu(display("update metadata needs a decryption key for slot {slot}"))]
    MissingUpdateKey { slot: usize },
    #[snafu(display("update metadata uses unsupported key slot {slot}"))]
    UnsupportedUpdateKey { slot: usize },
    #[snafu(display(
        "PKG entry 0x{id:08x} extends beyond the end of the file. The package is incomplete or damaged."
    ))]
    TruncatedEntry { id: u32 },
    #[snafu(transparent)]
    PackageFilesystem {
        source: crate::PackageFilesystemError,
    },
    #[snafu(display("cannot unlock update metadata"))]
    PasscodeMetadata {
        source: orbis_pkg::passcode::PasscodeValidationError,
    },
    #[snafu(display("installation filesystem operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("invalid installation metadata"))]
    Metadata { source: orbis_pkg::sfo::SfoError },
    #[snafu(display("update extraction failed"))]
    Extract { source: ExtractError },
    #[snafu(display("invalid installation state"))]
    State { source: serde_json::Error },
}
impl InstallError {
    fn missing_update_key(slot: usize) -> Self {
        if slot < orbis_pkg::passcode::ENTRY_KEY_COUNT {
            Self::MissingUpdateKey { slot }
        } else {
            Self::UnsupportedUpdateKey { slot }
        }
    }
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
impl From<crate::inventory::InventoryError> for InstallError {
    fn from(error: crate::inventory::InventoryError) -> Self {
        use crate::inventory::InventoryError;
        match error {
            InventoryError::Invalid { message } => Self::Invalid { message },
            InventoryError::Io { source } => Self::Io { source },
            InventoryError::Json { source } => Self::State { source },
            InventoryError::ManifestTooLarge => invalid("installation manifest is too large"),
            InventoryError::Cancelled => Self::Extract {
                source: ExtractError::Cancelled,
            },
        }
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub change: ChangeKind,
}

/// Where update files are installed. Separate mode preserves the original base.
#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallMode {
    #[default]
    Separate,
    Merge,
}

/// A read-only plan. Sizes are estimates; extraction can still run out of space.
#[derive(Debug, Serialize)]
pub struct InstallPlan {
    pub mode: InstallMode,
    pub backup_bytes: u64,
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

impl InstallPlan {
    /// Rebuilds the preview under the installation lock and applies it if the planned changes match.
    pub fn apply<R: AsRef<[u8]> + Sync>(
        &self,
        pkg: &Pkg<R>,
        base: &Path,
        progress: impl ExtractProgress,
    ) -> Result<InstallPlan> {
        run_install(pkg, base, self.mode, Some(self), progress)
    }

    fn matches_preview(&self, preview: &Self) -> bool {
        // Free space can change independently. Check the current value before staging.
        self.mode == preview.mode
            && self.title_id == preview.title_id
            && self.content_id == preview.content_id
            && self.current_version == preview.current_version
            && self.update_version == preview.update_version
            && self.destination == preview.destination
            && self.backup_bytes == preview.backup_bytes
            && self.extracted_bytes == preview.extracted_bytes
            && self.required_free_bytes == preview.required_free_bytes
            && self.files == preview.files
    }

    fn check_target(&self, layout: &Layout, id: &ContentId) -> Result<()> {
        if self.destination != layout.install_destination(self.mode)
            || self.content_id != id.as_str()
        {
            return Err(invalid(
                "The update destination or package changed after the preview. Check the update again before installing.",
            ));
        }
        let _guard = merge::read_lock(layout)?;
        layout.require_recovered()?;
        let base = layout.base_metadata()?;
        compatibility::check_identity(id, &base.title_id, &base.content_id)
    }
}

fn serialize_path<S: serde::Serializer>(
    path: &Path,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

/// Reads the original base's extracted PARAM.SFO without hashing installed updates.
pub fn base_metadata(base: &Path) -> Result<AppMetadata> {
    Layout::inspect(base)?.base_metadata()
}

/// Inspects compatibility and affected paths under the existing installation's shared lock.
/// Previewing an untouched game creates no files.
/// `pkg` must have the keys required to read its metadata and list its filesystem.
pub fn plan<R: AsRef<[u8]> + Sync>(pkg: &Pkg<R>, base: &Path) -> Result<InstallPlan> {
    plan_with_progress(pkg, base, SilentProgress)
}

/// Plans an update with progress and cooperative cancellation during inventory reads.
pub fn plan_with_progress<R: AsRef<[u8]> + Sync, P: ExtractProgress>(
    pkg: &Pkg<R>,
    base: &Path,
    progress: P,
) -> Result<InstallPlan> {
    let location = Layout::open(base)?;
    let _guard = merge::read_lock(&location)?;
    make_plan(pkg, &location, &progress)
}

/// Plans either a sibling update or a reversible merge into an original base.
pub fn plan_with_mode<R: AsRef<[u8]> + Sync, P: ExtractProgress>(
    pkg: &Pkg<R>,
    base: &Path,
    mode: InstallMode,
    progress: P,
) -> Result<InstallPlan> {
    match mode {
        InstallMode::Separate => plan_with_progress(pkg, base, progress),
        InstallMode::Merge => merge::plan(pkg, base, progress),
    }
}

/// Installs using the selected mode. Merge accepts one update; restore the base
/// with [`rollback`] before installing another update in either mode.
pub fn install_with_mode<R: AsRef<[u8]> + Sync, P: ExtractProgress>(
    pkg: &Pkg<R>,
    base: &Path,
    mode: InstallMode,
    progress: P,
) -> Result<InstallPlan> {
    run_install(pkg, base, mode, None, progress)
}

fn run_install<R: AsRef<[u8]> + Sync>(
    pkg: &Pkg<R>,
    base: &Path,
    mode: InstallMode,
    preview: Option<&InstallPlan>,
    progress: impl ExtractProgress,
) -> Result<InstallPlan> {
    let layout = Layout::open(base)?;
    if let Some(preview) = preview {
        preview.check_target(&layout, pkg.header().content_id())?;
    }
    let layout = layout.lock_for_write()?;
    let plan = match mode {
        InstallMode::Separate => make_plan(pkg, &layout, &progress)?,
        InstallMode::Merge => merge::make_plan(pkg, &layout, &progress)?,
    };
    if preview.is_some_and(|preview| !plan.matches_preview(preview)) {
        return Err(invalid(
            "The package or installed game changed after the preview. Check the update again before installing.",
        ));
    }
    check_space(plan.required_free_bytes, plan.available_bytes)?;
    match mode {
        InstallMode::Separate => install_separate(pkg, layout, plan, progress),
        InstallMode::Merge => merge::install_planned(pkg, layout, plan, progress),
    }
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
    check_update_identity(pkg, &base)?;
    let package = inspect_update(pkg);
    if let UpdateReadiness::NeedsPasscode(locked) = &package.readiness {
        locked.check_version(location, &base)?;
    }
    let update = package.readiness.into_ready()?;
    let state = location.state()?;
    let current = location.active_manifest(&state, &base, progress)?;
    location.verify_previous(&state, &base, progress)?;
    let current_version = current
        .as_ref()
        .map_or(Ok(base.version), |m| m.app_version.parse::<AppVersion>())?;
    check_update_version(update.version, current_version)?;
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
        mode: InstallMode::Separate,
        backup_bytes: 0,
        title_id: update.title_id,
        content_id: update.content_id,
        current_version: current_version.to_string(),
        update_version: update.version.to_string(),
        destination: location.install_destination(InstallMode::Separate),
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
/// Final sync failures after commit are reported through [`ExtractProgress::warning`].
pub fn install<R: AsRef<[u8]> + Sync, P: ExtractProgress>(
    pkg: &Pkg<R>,
    base: &Path,
    progress: P,
) -> Result<InstallPlan> {
    install_with_mode(pkg, base, InstallMode::Separate, progress)
}

fn install_separate<R: AsRef<[u8]> + Sync>(
    pkg: &Pkg<R>,
    location: Layout,
    plan: InstallPlan,
    progress: impl ExtractProgress,
) -> Result<InstallPlan> {
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
        location.activate(&progress)
    })();
    if let Err(error) = result {
        location.restore_after_error(&error, &progress)?;
        return Err(error);
    }
    Ok(plan)
}

/// Restores a merged game's originals. Refuses separate updates and pending
/// merges; use [`recover`] to finish an interrupted merge or restoration.
pub fn restore_merged(base: &Path, progress: impl ExtractProgress) -> Result<()> {
    GameTarget::open(base.to_owned()).restore_merged(&progress)
}

/// Recovers an interrupted activation/rollback and removes abandoned staging data.
/// With no pending transaction this is a harmless cleanup operation.
pub fn recover(base: &Path) -> Result<()> {
    recover_with_progress(base, SilentProgress)
}

/// Recovers with non-fatal warnings through [`ExtractProgress::warning`].
/// Recovery is not cancellable once directory restoration begins.
pub fn recover_with_progress<P: ExtractProgress>(base: &Path, progress: P) -> Result<()> {
    GameTarget::open(base.to_owned()).recover(&progress)
}

#[cfg(test)]
mod tests;
