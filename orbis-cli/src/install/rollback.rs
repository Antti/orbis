use super::{storage::RollbackTarget, *};

/// The separate update and restore point offered by a game-folder status check.
#[derive(Debug)]
pub struct SeparateRollback {
    title_id: String,
    base_version: String,
    active_version: String,
    previous_update: Option<String>,
}

impl InstallationStatus {
    /// Returns a confirmable rollback only for a healthy, managed separate update.
    pub fn separate_rollback(&self) -> Option<SeparateRollback> {
        if self.mode != InstallMode::Separate
            || self.pending_recovery
            || !self.issues.is_empty()
            || !self.modified_files.is_empty()
            || !self.unmanaged_directories.is_empty()
        {
            return None;
        }
        let version = self.rollback_version.as_ref()?;
        let previous_update = match self.rollback.as_str() {
            "base" if version == &self.base_version => None,
            "update" => Some(version.clone()),
            _ => return None,
        };
        Some(SeparateRollback {
            title_id: self.title_id.clone(),
            base_version: self.base_version.clone(),
            active_version: self.active_version.clone()?,
            previous_update,
        })
    }
}

impl SeparateRollback {
    pub fn active_version(&self) -> &str {
        &self.active_version
    }

    pub fn restores_base(&self) -> bool {
        self.previous_update.is_none()
    }

    pub fn target_version(&self) -> &str {
        self.previous_update
            .as_deref()
            .unwrap_or(&self.base_version)
    }

    /// Rechecks the confirmed versions under the installation lock before restoring.
    pub fn apply(&self, base: &Path, progress: impl ExtractProgress) -> Result<()> {
        let location = Layout::open(base, true)?;
        location.require_recovered()?;
        rollback_separate(&location, Some(self), &progress)
    }
}

/// Restores the previous managed update, or removes the first update to use the base.
/// Only one rollback is retained. Modified/unmanaged trees are never overwritten.
pub fn rollback(base: &Path) -> Result<()> {
    rollback_with_progress(base, SilentProgress)
}

/// Rolls back with cancellable checks of the active and previous file inventories.
/// Final sync failures after commit are reported through [`ExtractProgress::warning`].
pub fn rollback_with_progress(base: &Path, progress: impl ExtractProgress) -> Result<()> {
    let location = Layout::open(base, true)?;
    location.require_recovered()?;
    if merge::present(&location)? {
        return merge::restore(&location, &progress);
    }
    rollback_separate(&location, None, &progress)
}

fn rollback_separate(
    location: &Layout,
    expected: Option<&SeparateRollback>,
    progress: &impl ExtractProgress,
) -> Result<()> {
    let state = location.state()?;
    let base = location.base_metadata()?;
    let active = location
        .active_manifest(&state, &base, progress)?
        .map(|m| m.app_version);
    let previous = location
        .verify_previous(&state, &base, progress)?
        .map(|m| m.app_version);
    if state.rollback == RollbackTarget::None {
        return Err(invalid(
            "no previous installation is available for rollback",
        ));
    }
    if let Some(expected) = expected {
        if expected.title_id != base.title_id
            || expected.base_version != base.version.to_string()
            || active.as_deref() != Some(expected.active_version())
            || expected.previous_update != previous
        {
            return Err(invalid(
                "The installed versions changed. Refresh the game folder to review the new rollback target.",
            ));
        }
        let root = location.manager()?;
        // Status also reports uppercase legacy patches on case-sensitive volumes.
        let mut legacy_patch = location.base.clone();
        legacy_patch.push("-PATCH");
        if storage::exists(&location.parent, legacy_patch)?
            || storage::exists(root, "staging")?
            || storage::exists(root, "retired")?
        {
            return Err(invalid(
                "The installation needs attention. Refresh the game folder to review its status.",
            ));
        }
    }
    if progress.is_cancelled() {
        return Err(ExtractError::Cancelled.into());
    }
    location.clean_uncommitted()?;
    if let Err(error) = location.rollback_transaction(progress) {
        location.restore_after_error(&error, progress)?;
        return Err(error);
    }
    Ok(())
}
