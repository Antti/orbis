use super::{Action, OrbisApp, UpdateBase};
use crate::operations::{self, Operation, PackageInfo, PackageSource, UpdatePackage};
use orbis_cli::install::{InstallError, InstallMode, InstallPlan, UpdateReadiness};

enum UpdateStep<'a> {
    CheckUpdate,
    CheckTarget,
    EnterPasscode,
    Install(&'a InstallPlan),
    PackageRejected(&'a InstallError),
    TargetRejected(&'a InstallError),
}

/// The last check of the selected package against this folder and install mode.
#[derive(Default)]
pub(super) enum UpdateTargetState {
    #[default]
    Unchecked,
    AwaitingPasscode,
    Rejected(InstallError),
    Preview(Box<InstallPlan>),
}

impl UpdateTargetState {
    pub(super) fn preview(&self) -> Option<&InstallPlan> {
        match self {
            Self::Preview(plan) => Some(plan),
            _ => None,
        }
    }

    pub(super) fn discard_preview(&mut self) {
        if matches!(self, Self::Preview(_)) {
            *self = Self::Unchecked;
        }
    }

    pub(super) fn controls<'a>(
        &'a self,
        info: &'a PackageInfo,
        base: &'a UpdateBase,
    ) -> UpdateControls<'a> {
        let step = match self {
            Self::Preview(plan) => UpdateStep::Install(plan),
            Self::Rejected(error) => UpdateStep::TargetRejected(error),
            Self::AwaitingPasscode => UpdateStep::EnterPasscode,
            Self::Unchecked => match &info.update.readiness {
                UpdateReadiness::Ready(_) => UpdateStep::CheckUpdate,
                UpdateReadiness::Rejected(error) => UpdateStep::PackageRejected(error),
                UpdateReadiness::NeedsPasscode(_) if base.path().is_some() => {
                    UpdateStep::CheckTarget
                }
                UpdateReadiness::NeedsPasscode(_) => UpdateStep::EnterPasscode,
            },
        };
        UpdateControls { step, base, info }
    }
}

pub(super) struct UpdateControls<'a> {
    step: UpdateStep<'a>,
    base: &'a UpdateBase,
    info: &'a PackageInfo,
}

#[derive(Debug)]
pub(super) enum PackageGuidance {
    ChooseAnotherPackage(String),
    Settings {
        warning: Option<String>,
        passcode: PasscodePrompt,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PasscodePrompt {
    Hidden,
    Optional,
    Required,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UpdateCommand {
    Unavailable,
    Check,
    Retry,
    Install(InstallMode),
}

impl UpdateCommand {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Unavailable | Self::Check => "Check update",
            Self::Retry => "Check again",
            Self::Install(InstallMode::Separate) => "Install update",
            Self::Install(InstallMode::Merge) => "Merge update",
        }
    }
}

impl UpdateControls<'_> {
    pub(super) fn command(&self) -> UpdateCommand {
        if self.base.path().is_none() {
            return UpdateCommand::Unavailable;
        }
        match self.step {
            UpdateStep::Install(plan) if plan.has_sufficient_space() => {
                UpdateCommand::Install(plan.mode)
            }
            UpdateStep::Install(_) | UpdateStep::TargetRejected(_) => UpdateCommand::Retry,
            UpdateStep::CheckUpdate | UpdateStep::CheckTarget => UpdateCommand::Check,
            UpdateStep::EnterPasscode | UpdateStep::PackageRejected(_) => {
                UpdateCommand::Unavailable
            }
        }
    }

    pub(super) fn guidance(
        &self,
        action: Action,
        metadata_only: bool,
        passcode_validated: bool,
    ) -> PackageGuidance {
        use PasscodePrompt::{Hidden, Optional, Required};
        let installing = action == Action::Update;
        let inspector_prompt =
            if !installing && (self.info.can_unlock_details || passcode_validated) {
                Optional
            } else {
                Hidden
            };
        let (rejection, passcode) = match self.step {
            UpdateStep::PackageRejected(error @ InstallError::PackageFilesystem { .. }) => {
                let prompt = if action == Action::Verify && metadata_only {
                    Optional
                } else {
                    inspector_prompt
                };
                (Some(error), prompt)
            }
            UpdateStep::PackageRejected(error @ InstallError::TruncatedEntry { .. }) => {
                (Some(error), inspector_prompt)
            }
            UpdateStep::PackageRejected(error) | UpdateStep::TargetRejected(error)
                if installing =>
            {
                (Some(error), Hidden)
            }
            UpdateStep::PackageRejected(InstallError::PasscodeMetadata { .. }) => {
                (None, inspector_prompt)
            }
            UpdateStep::CheckTarget if installing => (None, Hidden),
            UpdateStep::EnterPasscode if installing => (None, Required),
            _ => (None, Optional),
        };
        let warning = match rejection {
            Some(error) => {
                let reason = operations::Failure::from_error(error).details;
                if matches!(self.step, UpdateStep::PackageRejected(_))
                    && matches!(self.base, UpdateBase::Game(_))
                {
                    return PackageGuidance::ChooseAnotherPackage(reason);
                }
                Some(reason)
            }
            None => self.missing_key_warning(installing),
        };
        PackageGuidance::Settings { warning, passcode }
    }

    fn missing_key_warning(&self, installing: bool) -> Option<String> {
        let missing_keys = self.info.update.missing_keys;
        if missing_keys == 0 {
            return None;
        }
        let entries = if missing_keys == 1 {
            "entry"
        } else {
            "entries"
        };
        let guidance = match (&self.step, installing) {
            (UpdateStep::PackageRejected(InstallError::UnsupportedUpdateKey { slot }), _) => {
                format!("Key slot {slot} is unsupported. Extraction may be incomplete.")
            }
            (UpdateStep::PackageRejected(error @ InstallError::PasscodeMetadata { .. }), _) => {
                operations::Failure::from_error(*error).details
            }
            (UpdateStep::EnterPasscode, true) => {
                "Validate the package passcode before installing this update.".into()
            }
            (UpdateStep::CheckTarget, true) => {
                "Check the selected game before entering a passcode.".into()
            }
            _ => "Extraction may be incomplete.".into(),
        };
        Some(format!(
            "{} encrypted metadata {entries} cannot be read. {guidance}",
            missing_keys
        ))
    }
}

impl OrbisApp {
    pub(super) fn update_target_changed(&mut self) {
        if let Some(info) = &self.package
            && self.action == Action::Update
            && (matches!(self.base, UpdateBase::Game(_))
                || matches!(info.update.readiness, UpdateReadiness::NeedsPasscode(_)))
        {
            self.check_update();
        } else {
            self.update_target = UpdateTargetState::Unchecked;
        }
    }

    fn check_update(&mut self) {
        self.update_target = UpdateTargetState::Unchecked;
        let (Some(info), Some(base)) = (&self.package, self.base.path().cloned()) else {
            return;
        };
        let mode = self.mode;
        let package = match info.update.readiness {
            UpdateReadiness::Ready(_) => UpdatePackage::Readable {
                source: PackageSource::new(info.path.clone(), self.passcode.clone()),
                id: info.content_id,
            },
            UpdateReadiness::NeedsPasscode(update) => UpdatePackage::Locked(update),
            UpdateReadiness::Rejected(_) => return,
        };
        self.start(Operation::PreviewUpdate {
            package,
            base,
            mode,
        });
    }

    pub(super) fn submit_update(&mut self, command: UpdateCommand) {
        match command {
            UpdateCommand::Check | UpdateCommand::Retry => self.check_update(),
            UpdateCommand::Install(_) => {
                if let (Some(info), Some(base)) = (&self.package, self.base.path())
                    && let UpdateTargetState::Preview(preview) =
                        std::mem::take(&mut self.update_target)
                {
                    self.start(Operation::InstallUpdate {
                        source: PackageSource::new(info.path.clone(), self.passcode.clone()),
                        base: base.clone(),
                        preview,
                    });
                }
            }
            UpdateCommand::Unavailable => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{common, features, operations::Outcome};
    use orbis_cli::SilentProgress;
    use std::{fs, path::Path};

    fn inspect(dir: &Path, bytes: &[u8]) -> Box<PackageInfo> {
        let path = dir.join("update.pkg");
        fs::write(&path, bytes).unwrap();
        let operation = Operation::Inspect(PackageSource::new(path, None));
        let Outcome::Inspected(info) = operations::run(operation, &SilentProgress).unwrap() else {
            panic!("expected package inspection");
        };
        info
    }

    #[test]
    fn a_truncated_entry_allows_unlocking_details_only_outside_installation() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = features::passcoded_pkg(2);
        let end = bytes.len() as u32;
        common::be32(&mut bytes, 0x1030, end); // PfsImageKey is truncated, PARAM.SFO is intact.
        let info = inspect(dir.path(), &bytes);
        let state = UpdateTargetState::Unchecked;
        let picked = UpdateBase::Pick(Some(dir.path().to_owned()));
        let controls = state.controls(&info, &picked);
        assert_eq!(controls.command(), UpdateCommand::Unavailable);
        let PackageGuidance::Settings {
            warning: Some(reason),
            passcode: PasscodePrompt::Optional,
        } = controls.guidance(Action::Extract, false, false)
        else {
            panic!("expected a damage warning and an optional inspector passcode");
        };
        let game = UpdateBase::Game(dir.path().to_owned());
        assert!(matches!(
            state.controls(&info, &game).guidance(Action::Update, false, false),
            PackageGuidance::ChooseAnotherPackage(message) if message == reason
        ));
    }

    #[test]
    fn checking_a_locked_update_determines_whether_to_request_a_passcode_or_retry() {
        let dir = tempfile::tempdir().unwrap();
        let info = inspect(dir.path(), &features::passcoded_pkg(2));
        let game = UpdateBase::Game(dir.path().to_owned());
        for (state, command, prompt) in [
            (
                UpdateTargetState::Unchecked,
                UpdateCommand::Check,
                PasscodePrompt::Hidden,
            ),
            (
                UpdateTargetState::AwaitingPasscode,
                UpdateCommand::Unavailable,
                PasscodePrompt::Required,
            ),
            (
                UpdateTargetState::Rejected(InstallError::UpdateIdentityMismatch),
                UpdateCommand::Retry,
                PasscodePrompt::Hidden,
            ),
        ] {
            let controls = state.controls(&info, &game);
            assert_eq!(controls.command(), command);
            assert!(matches!(
                controls.guidance(Action::Update, false, false),
                PackageGuidance::Settings { passcode, .. } if passcode == prompt
            ));
        }
    }
}
