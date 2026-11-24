use super::*;

/// What can be established about an update before opening its filesystem.
#[derive(Debug)]
pub struct UpdateCheck {
    metadata: Result<AppMetadata>,
    pub missing_keys: usize,
    highest_missing_slot: Option<usize>,
    target_error: Option<InstallError>,
}

impl UpdateCheck {
    /// Package metadata alone controls whether the standalone update tab is available.
    pub fn package_error(&self) -> Option<&InstallError> {
        self.metadata.as_ref().err()
    }

    /// A reason that entering a passcode for this package and game cannot resolve.
    pub fn rejection(&self) -> Option<&InstallError> {
        self.package_error()
            .filter(|error| !matches!(error, InstallError::MissingUpdateKey { .. }))
            .or(self.target_error.as_ref())
    }

    pub fn needs_passcode(&self) -> bool {
        self.missing_keys > 0 && self.rejection().is_none()
    }

    pub fn can_check_update(&self) -> bool {
        self.metadata.is_ok() && self.missing_keys == 0 && self.target_error.is_none()
    }

    /// Requires every metadata entry to be readable before opening the filesystem.
    pub fn into_ready(self) -> Result<AppMetadata> {
        if let Some(error) = self.target_error {
            return Err(error);
        }
        let metadata = self.metadata?;
        if let Some(slot) = self.highest_missing_slot {
            return Err(InstallError::missing_update_key(slot));
        }
        Ok(metadata)
    }
}

/// Checks all locked entries together. When a passcode might help, check the
/// selected game using only its installation metadata. Preview verifies file contents.
pub fn inspect_update<R: AsRef<[u8]>>(pkg: &Pkg<R>, base: Option<&Path>) -> UpdateCheck {
    inspect_with_target(pkg, |version| {
        let Some(path) = base else { return Ok(()) };
        let location = Layout::open(path, false)?;
        let _guard = super::merge::read_lock(&location)?;
        location.require_recovered()?;
        let base = location.base_metadata()?;
        check_update_identity(pkg, &base)?;
        check_locked_version(&location, &base, version)
    })
}

// Defer opening the target until a passcode could change the outcome. The full
// planner supplies its existing layout and lock when it calls this check.
pub(super) fn inspect_with_target<R: AsRef<[u8]>>(
    pkg: &Pkg<R>,
    check_target: impl FnOnce(Option<AppVersion>) -> Result<()>,
) -> UpdateCheck {
    let mut missing_keys = 0;
    let mut highest_missing_slot = None;
    // param_sfo_data reports malformed tables. Iterating past the first invalid
    // entry could walk billions of absent entries from a corrupt header.
    for (_, entry) in pkg.entries().map_while(|entry| entry.ok()) {
        let slot = entry.key_index();
        if entry.is_encrypted() && !pkg.has_entry_key(slot) {
            missing_keys += 1;
            highest_missing_slot = highest_missing_slot.max(Some(slot));
        }
    }
    let mut metadata = update_metadata(pkg);
    if let Some(slot) = highest_missing_slot.filter(|&s| s >= orbis_pkg::passcode::ENTRY_KEY_COUNT)
        && matches!(
            metadata,
            Ok(_)
                | Err(InstallError::MissingUpdateKey { .. }
                    | InstallError::UnsupportedUpdateKey { .. })
        )
    {
        metadata = Err(InstallError::UnsupportedUpdateKey { slot });
    }
    let mut check = UpdateCheck {
        metadata,
        missing_keys,
        highest_missing_slot,
        target_error: None,
    };
    if check.needs_passcode() {
        let version = check.metadata.as_ref().ok().map(|m| m.version);
        check.target_error = check_target(version).err();
    }
    check
}

pub(super) fn check_locked_version(
    location: &Layout,
    base: &AppMetadata,
    version: Option<AppVersion>,
) -> Result<()> {
    let active = location.open_active_manifest(&location.state()?)?;
    let installed = match active {
        Some((dir, manifest)) => storage::validate_manifest_metadata(&dir, &manifest, base)?,
        None => base.version,
    };
    if let Some(version) = version {
        check_update_version(version, installed)?;
    }
    Ok(())
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
    let id = pkg.header().content_id();
    if id.as_str() != base.content_id || id.title_id() != base.title_id {
        return Err(invalid(
            "update does not match the base game's title and content ID",
        ));
    }
    Ok(())
}
