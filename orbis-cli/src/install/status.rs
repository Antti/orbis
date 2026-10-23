//! Diagnose installations without creating state or attempting recovery.
use super::{
    Result,
    storage::{self, Layout, Manifest, RollbackTarget},
};
use crate::ExtractProgress;
use fs2::FileExt;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct InstallationStatus {
    pub mode: super::InstallMode,
    pub schema_version: u32,
    pub title_id: String,
    pub base_version: String,
    pub active_version: Option<String>,
    pub rollback: String,
    pub rollback_version: Option<String>,
    pub pending_recovery: bool,
    pub unmanaged_directories: Vec<PathBuf>,
    pub modified_files: Vec<String>,
    pub issues: Vec<String>,
    /// Untouched base files are not tracked; merge mode checks replaced paths.
    pub base_integrity: String,
}
struct ReadLock(std::fs::File);
impl Drop for ReadLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub fn status(base: &Path, progress: impl ExtractProgress) -> Result<InstallationStatus> {
    if let Some(report) = super::merge::status(base, &progress)? {
        return Ok(report);
    }
    let layout = Layout::inspect(base)?;
    let metadata = layout.base_metadata()?;
    let mut report = InstallationStatus {
        schema_version: 1,
        mode: super::InstallMode::Separate,
        title_id: metadata.title_id.clone(),
        base_version: metadata.version.to_string(),
        active_version: None,
        rollback: "none".into(),
        rollback_version: None,
        pending_recovery: false,
        unmanaged_directories: Vec::new(),
        modified_files: Vec::new(),
        issues: Vec::new(),
        base_integrity: "not_tracked".into(),
    };
    // Lock only an existing lock file. An untouched base remains untouched.
    let _guard = if let Some(root) = &layout.root {
        if storage::exists(root, "lock")? {
            if !root.symlink_metadata("lock")?.is_file() {
                report
                    .issues
                    .push("installation lock is not a regular file".into());
                return Ok(report);
            }
            let file = root.open("lock")?.into_std();
            if FileExt::try_lock_shared(&file).is_err() {
                report.issues.push(
                    "installation operation is in progress; retry status when it finishes".into(),
                );
                return Ok(report);
            }
            Some(ReadLock(file))
        } else {
            None
        }
    } else {
        None
    };
    for suffix in ["-PATCH", "-patch"] {
        let mut name = layout.base.clone();
        name.push(suffix);
        if storage::exists(&layout.parent, &name)? {
            let path = layout.parent_path.join(name);
            if !report.unmanaged_directories.contains(&path) {
                report.unmanaged_directories.push(path);
            }
        }
    }
    if let Some(root) = &layout.root {
        report.pending_recovery = storage::exists(root, "journal.json")?;
        if report.pending_recovery {
            report.issues.push(
                "pending installation journal; run recover --base <DIR> with the emulator stopped"
                    .into(),
            );
        }
        for path in ["staging", "retired"] {
            if storage::exists(root, path)? {
                report.issues.push(format!(
                    "abandoned {path} directory; run recover --base <DIR>"
                ));
            }
        }
    }
    let state = match layout.state() {
        Ok(s) => s,
        Err(e) => {
            report.issues.push(e.to_string());
            return Ok(report);
        }
    };
    report.rollback = match state.rollback {
        RollbackTarget::None => "none",
        RollbackTarget::Base => "base",
        RollbackTarget::Update => "update",
    }
    .into();
    if state.rollback == RollbackTarget::Base {
        report.rollback_version = Some(report.base_version.clone());
    }
    let present = storage::exists(&layout.parent, &layout.active)?;
    if present && !state.active {
        report.unmanaged_directories.push(layout.active_path());
    }
    if state.active && !present {
        report
            .issues
            .push("managed -UPDATE directory is missing".into());
    }
    if state.active && present {
        let checked = (|| -> Result<()> {
            storage::require_directory(&layout.parent, &layout.active)?;
            let dir = layout.parent.open_dir(&layout.active)?;
            let manifest: Manifest = storage::read_json(&dir, storage::MANIFEST)?;
            report.active_version = Some(manifest.app_version.clone());
            let differences = storage::compare_inventory(&dir, &manifest, &progress)?;
            report.modified_files = differences.modified_files;
            if !differences.matches {
                report
                    .issues
                    .push("managed update inventory differs from its manifest".into());
            }
            if let Err(error) = storage::validate_manifest_metadata(&dir, &manifest, &metadata) {
                match error {
                    super::InstallError::Invalid { .. } => report
                        .issues
                        .push("managed update metadata is inconsistent".into()),
                    other => return Err(other),
                }
            }
            Ok(())
        })();
        if let Err(error) = checked {
            if progress.is_cancelled() {
                return Err(error);
            }
            report.issues.push(error.to_string());
        }
    }
    match layout.verify_previous(&state, &metadata, &progress) {
        Ok(Some(manifest)) => report.rollback_version = Some(manifest.app_version),
        Ok(None) => {}
        Err(error) if progress.is_cancelled() => return Err(error),
        Err(error) => report.issues.push(format!("rollback: {error}")),
    }
    Ok(report)
}
