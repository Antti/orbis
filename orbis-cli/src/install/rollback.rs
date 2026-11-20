use super::{storage::RollbackTarget, *};
use crate::fs_output::directory_identity;
use sha2::{Digest, Sha256};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct VerifiedUpdate {
    version: String,
    fingerprint: [u8; 32],
}

impl VerifiedUpdate {
    pub(super) fn from_manifest(manifest: Manifest) -> Result<Self> {
        // The manifest includes the game identity and ordered paths, sizes and file hashes.
        let mut hash = Sha256::new();
        serde_json::to_writer(&mut hash, &manifest)?;
        Ok(Self {
            version: manifest.app_version,
            fingerprint: hash.finalize().into(),
        })
    }
}

/// The separate update and restore point offered by a game-folder status check.
#[derive(Debug)]
pub struct SeparateRollback {
    base_path: PathBuf,
    base_identity: (u64, u64),
    base_version: String,
    active: VerifiedUpdate,
    previous: Option<VerifiedUpdate>,
}

impl SeparateRollback {
    pub(super) fn new(
        layout: &Layout,
        base_dir: &Dir,
        base_version: String,
        active: VerifiedUpdate,
        previous: Option<Manifest>,
    ) -> Result<Self> {
        Ok(Self {
            base_path: layout.parent_path.join(&layout.base),
            base_identity: directory_identity(base_dir)?,
            base_version,
            active,
            previous: previous.map(VerifiedUpdate::from_manifest).transpose()?,
        })
    }

    pub fn active_version(&self) -> &str {
        &self.active.version
    }

    pub fn restores_base(&self) -> bool {
        self.previous.is_none()
    }

    pub fn target_version(&self) -> &str {
        self.previous
            .as_ref()
            .map_or(&self.base_version, |update| &update.version)
    }

    /// Rechecks the scanned game folder and both verified manifests under its installation lock.
    pub fn apply(&self, base: &Path, progress: impl ExtractProgress) -> Result<()> {
        let location = Layout::open_locked_existing(base)?;
        if location.parent_path.join(&location.base) != self.base_path
            || directory_identity(&location.base_dir()?)? != self.base_identity
        {
            return Err(invalid(
                "The game folder changed. Refresh it before restoring.",
            ));
        }
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
pub fn rollback_with_progress<P: ExtractProgress>(base: &Path, progress: P) -> Result<()> {
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
        .filter(|_| expected.is_some())
        .map(VerifiedUpdate::from_manifest)
        .transpose()?;
    let previous = location
        .verify_previous(&state, &base, progress)?
        .filter(|_| expected.is_some())
        .map(VerifiedUpdate::from_manifest)
        .transpose()?;
    if state.rollback == RollbackTarget::None {
        return Err(invalid(
            "no previous installation is available for rollback",
        ));
    }
    if let Some(expected) = expected {
        if expected.base_version != base.version.to_string()
            || active.as_ref() != Some(&expected.active)
            || previous != expected.previous
        {
            return Err(invalid(
                "The installation changed. Refresh the game folder to review the rollback target.",
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
