use super::view::{metric, path_label, warning};
use super::*;
use crate::theme::{self, heading, muted};
use orbis_cli::install::{InstallationStatus, SeparateRollback};

pub(super) struct GameFolder {
    pub base: PathBuf,
    // A mutation invalidates the last scan even if it stops partway through.
    pub status: Option<Box<InstallationStatus>>,
    rollback: Option<SeparateRollback>,
    confirming_rollback: bool,
}

impl GameFolder {
    pub(super) fn new(
        base: PathBuf,
        status: Option<Box<InstallationStatus>>,
        rollback: Option<SeparateRollback>,
    ) -> Self {
        Self {
            base,
            status,
            rollback,
            confirming_rollback: false,
        }
    }

    pub fn show(&self, ui: &mut egui::Ui) {
        path_label(ui, &self.base, "");
        let Some(status) = &self.status else { return };
        let needs_attention = !status.issues.is_empty() || !status.unmanaged_directories.is_empty();
        let (mode, active) = match (status.mode, status.active_version.as_deref()) {
            _ if status.pending_recovery => ("Interrupted installation", "Unknown"),
            (InstallMode::Merge, version) => {
                ("Merged into game folder", version.unwrap_or("Unknown"))
            }
            (InstallMode::Separate, Some(version)) => ("Separate update", version),
            (InstallMode::Separate, None) if needs_attention => {
                ("Installation needs attention", "Unknown")
            }
            (InstallMode::Separate, None) => ("Base game only", status.base_version.as_str()),
        };
        ui.add_space(12.0);
        ui.label(heading(&status.title_id, 17.0));
        ui.label(mode);
        ui.add_space(10.0);
        ui.columns(2, |columns| {
            metric(&mut columns[0], "Base version", &status.base_version);
            metric(&mut columns[1], "Active version", active);
        });
        if let Some(version) = &status.rollback_version {
            ui.add_space(6.0);
            let target = if status.rollback == "base" {
                "Base"
            } else {
                "Update"
            };
            ui.label(format!("Rollback target: {target} {version}"));
        }
        if status.mode == InstallMode::Merge {
            ui.add_space(8.0);
            ui.label(if status.pending_recovery {
                "Close the emulator and recover the original game files before using this folder."
            } else {
                "Restore base puts the original files back and removes update-only files. Close the emulator before restoring."
            });
            ui.label(
                muted(
                    ui,
                    "Keep the adjacent .<game>.orbis-install backup folder with your game.",
                )
                .size(12.0),
            );
        }
        for issue in &status.issues {
            warning(ui, issue);
        }
        if !status.unmanaged_directories.is_empty() {
            ui.label(heading("Unmanaged update folders", 14.0));
            for path in &status.unmanaged_directories {
                path_label(ui, path, "");
            }
        }
        if !status.modified_files.is_empty() {
            egui::CollapsingHeader::new(format!(
                "Modified files ({})",
                status.modified_files.len()
            ))
            .show(ui, |ui| {
                for path in status.modified_files.iter().take(100) {
                    ui.label(path);
                }
                if status.modified_files.len() > 100 {
                    ui.label("Showing the first 100 files.");
                }
            });
        }
    }
}

impl OrbisApp {
    pub(super) fn game_view(&mut self, ui: &mut egui::Ui) {
        let base = self.game.as_ref().unwrap().base.clone();
        ui.horizontal(|ui| {
            ui.label(heading("Game folder", 21.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(self.job.is_none() && self.notice.is_none(), |ui| {
                    if ui.button("Back").clicked() {
                        self.game = None;
                        self.update_target_changed();
                    } else if ui.button("Refresh").clicked() {
                        self.open_game(base);
                    }
                });
            });
        });
        if let Some(game) = &self.game {
            game.show(ui);
        }
    }

    pub(super) fn open_game(&mut self, base: PathBuf) {
        if matches!(self.base, UpdateBase::Game(_)) {
            self.reset_package(UpdateBase::default());
        }
        self.export = None;
        self.preview = None;
        self.game = Some(GameFolder::new(base.clone(), None, None));
        self.start(PathBuf::new(), Operation::CheckGame { base });
    }

    pub(super) fn game_footer(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let game = self.game.as_mut().unwrap();
        let base = game.base.clone();
        let state = game.status.as_ref().map(|s| (s.mode, s.pending_recovery));
        let healthy = game.status.as_ref().is_some_and(|s| {
            s.issues.is_empty() && s.unmanaged_directories.is_empty() && s.modified_files.is_empty()
        });
        if let Some(plan) = &game.rollback
            && game.confirming_rollback
        {
            let target = if plan.restores_base() {
                "base game"
            } else {
                "previous update"
            };
            ui.label(heading(
                format!(
                    "Switch from {} to {target} {}?",
                    plan.active_version(),
                    plan.target_version()
                ),
                15.0,
            ));
            ui.label("Close the emulator first. Restoring removes the active update and uses the only rollback point.");
            let confirm = ui
                .horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        game.confirming_rollback = false;
                    }
                    ui.add(theme::primary("Confirm restore")).clicked()
                })
                .inner;
            if confirm {
                let plan = game.rollback.take().unwrap();
                game.status = None;
                game.confirming_rollback = false;
                self.start(PathBuf::new(), Operation::RollbackSeparate { base, plan });
            }
            return;
        }
        ui.horizontal_wrapped(|ui| match state {
            Some((InstallMode::Separate, false)) => {
                if healthy && ui.add(theme::primary("Install update…")).clicked() {
                    self.choose_package(frame, UpdateBase::Game(base.clone()));
                    return;
                }
                if ui.button("Export this game…").clicked() {
                    self.export = Some(tools::ExportState::from_base(base.clone()));
                }
                let game = self.game.as_mut().unwrap();
                if let Some(plan) = &game.rollback {
                    let target = if plan.restores_base() {
                        "base"
                    } else {
                        "previous update"
                    };
                    if ui
                        .button(format!("Restore {target} {}…", plan.target_version()))
                        .clicked()
                    {
                        game.confirming_rollback = true;
                    }
                }
            }
            Some((InstallMode::Merge, pending)) => {
                let label = if pending {
                    "Recover base"
                } else {
                    "Restore base"
                };
                if ui.add(theme::primary(label)).clicked() {
                    self.game.as_mut().unwrap().status = None;
                    let operation = if pending {
                        Operation::RecoverBase { base: base.clone() }
                    } else {
                        Operation::RestoreBase { base: base.clone() }
                    };
                    self.start(PathBuf::new(), operation);
                }
            }
            _ => {}
        });
    }
}
