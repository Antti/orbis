use super::*;
use orbis_pkg::header::ContentId;

/// Package facts stay valid when the user changes the target folder or installation mode.
#[derive(Debug)]
pub struct UpdateCheck {
    pub missing_keys: usize,
    pub readiness: UpdateReadiness,
}

/// Readiness for a full install preview, independent of the selected game.
#[derive(Debug)]
pub enum UpdateReadiness {
    Ready(AppMetadata),
    NeedsPasscode(LockedUpdate),
    Rejected(InstallError),
}

impl UpdateReadiness {
    pub fn rejection(&self) -> Option<&InstallError> {
        match self {
            Self::Rejected(error) => Some(error),
            _ => None,
        }
    }

    /// Whether a readable PARAM.SFO identifies a supported update.
    pub fn has_update_metadata(&self) -> bool {
        match self {
            Self::Ready(_) => true,
            Self::NeedsPasscode(locked) => locked.version.is_some(),
            Self::Rejected(_) => false,
        }
    }

    /// Require readable entries before opening the filesystem. For locked updates,
    /// report any known target conflict first. The full planner checks the unlocked PKG.
    pub fn require_readable_for(self, base: &Path, mode: InstallMode) -> Result<AppMetadata> {
        if let Self::NeedsPasscode(locked) = &self {
            locked.check_target(base, mode)?;
        }
        self.into_ready()
    }

    pub(super) fn into_ready(self) -> Result<AppMetadata> {
        match self {
            Self::Ready(metadata) => Ok(metadata),
            Self::NeedsPasscode(locked) => {
                Err(InstallError::MissingUpdateKey { slot: locked.slot })
            }
            Self::Rejected(error) => Err(error),
        }
    }
}

/// What can be checked against a game before the remaining entries are unlocked.
#[derive(Debug, Clone, Copy)]
pub struct LockedUpdate {
    id: ContentId,
    version: Option<AppVersion>,
    slot: usize,
}

impl LockedUpdate {
    /// Reads installation metadata under a shared lock. Full preview verifies file contents.
    pub fn check_target(&self, path: &Path, mode: InstallMode) -> Result<()> {
        let location = Layout::open(path, false)?;
        let _guard = super::merge::read_lock(&location)?;
        location.require_recovered()?;
        if mode == InstallMode::Merge {
            super::merge::require_original_base(&location)?;
        }
        let base = location.base_metadata()?;
        check_identity(&self.id, &base)?;
        self.check_version(&location, &base)
    }

    pub(super) fn check_version(&self, location: &Layout, base: &AppMetadata) -> Result<()> {
        let active = location.open_active_manifest(&location.state()?)?;
        let installed = match active {
            Some((dir, manifest)) => storage::validate_manifest_metadata(&dir, &manifest, base)?,
            None => base.version,
        };
        if let Some(version) = self.version {
            check_update_version(version, installed)?;
        }
        Ok(())
    }
}

/// Classifies all missing keys before suggesting a passcode. Malformed tables are
/// reported by update_metadata, which also stops at the first invalid entry.
pub fn inspect_update<R: AsRef<[u8]>>(pkg: &Pkg<R>) -> UpdateCheck {
    let mut missing_keys = 0;
    let mut highest_slot = None;
    for (_, entry) in pkg.entries().map_while(|entry| entry.ok()) {
        let slot = entry.key_index();
        if entry.is_encrypted() && !pkg.has_entry_key(slot) {
            missing_keys += 1;
            highest_slot = highest_slot.max(Some(slot));
        }
    }
    UpdateCheck {
        missing_keys,
        readiness: classify_update(pkg, highest_slot),
    }
}

fn classify_update<R: AsRef<[u8]>>(pkg: &Pkg<R>, missing_slot: Option<usize>) -> UpdateReadiness {
    let metadata = update_metadata(pkg);
    let Some(slot) = missing_slot else {
        return match metadata {
            Ok(metadata) => UpdateReadiness::Ready(metadata),
            Err(error) => UpdateReadiness::Rejected(error),
        };
    };
    let version = match metadata {
        Ok(metadata) => Some(metadata.version),
        Err(InstallError::MissingUpdateKey { .. } | InstallError::UnsupportedUpdateKey { .. }) => {
            None
        }
        Err(error) => return UpdateReadiness::Rejected(error),
    };
    if slot >= orbis_pkg::passcode::ENTRY_KEY_COUNT {
        return UpdateReadiness::Rejected(InstallError::UnsupportedUpdateKey { slot });
    }
    match pkg.check_passcode_metadata() {
        Ok(()) => UpdateReadiness::NeedsPasscode(LockedUpdate {
            id: *pkg.header().content_id(),
            version,
            slot,
        }),
        Err(source) => UpdateReadiness::Rejected(InstallError::PasscodeMetadata { source }),
    }
}

pub(super) fn check_update_version(update: AppVersion, installed: AppVersion) -> Result<()> {
    if update <= installed {
        return Err(invalid(format!(
            "update {update} is not newer than installed {installed}; repeated installs and downgrades are refused"
        )));
    }
    Ok(())
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
    let data = pkg
        .param_sfo_data()
        .map_err(|e| match e {
            ParamSfoError::ReadData {
                source: EntryDataError::NoDecryptionKey { key_index },
            } => InstallError::missing_update_key(key_index),
            other => invalid(other.to_string()),
        })?
        .ok_or_else(|| invalid("update has no PARAM.SFO"))?;
    let metadata = AppMetadata::parse(&data)?;
    if metadata.category != "gp" {
        return Err(invalid("update PARAM.SFO CATEGORY must be gp"));
    }
    if metadata.content_id != header.content_id().as_str() {
        return Err(invalid("PKG header and PARAM.SFO content IDs disagree"));
    }
    Ok(metadata)
}

/// Checks the header against the extracted base even when PARAM.SFO is locked.
pub fn check_update_identity<R: AsRef<[u8]>>(pkg: &Pkg<R>, base: &AppMetadata) -> Result<()> {
    check_identity(pkg.header().content_id(), base)
}

fn check_identity(id: &ContentId, base: &AppMetadata) -> Result<()> {
    if id.as_str() != base.content_id || id.title_id() != base.title_id {
        return Err(invalid(
            "update does not match the base game's title and content ID",
        ));
    }
    Ok(())
}
