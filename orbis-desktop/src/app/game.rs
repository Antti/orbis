use super::view::{metric, path_label, warning};
use super::*;
use crate::theme::{self, heading, muted};
use orbis_cli::install::InstallationStatus;

pub(super) struct GameFolder {
    pub base: PathBuf,
    // A mutation invalidates the last scan even if it stops partway through.
    pub status: Option<Box<InstallationStatus>>,
}

impl GameFolder {
    pub fn show(&self, ui: &mut egui::Ui) {
        ui.label(heading("Game folder", 21.0));
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
    pub(super) fn open_game(&mut self, base: PathBuf) {
        self.export = None;
        self.preview = None;
        self.game = Some(GameFolder {
            base: base.clone(),
            status: None,
        });
        self.start(PathBuf::new(), Operation::CheckGame { base });
    }

    pub(super) fn game_footer(&mut self, ui: &mut egui::Ui) {
        let game = self.game.as_ref().unwrap();
        let base = game.base.clone();
        let state = game.status.as_ref().map(|s| (s.mode, s.pending_recovery));
        ui.horizontal_wrapped(|ui| {
            match state {
                Some((InstallMode::Separate, false)) => {
                    if ui.add(theme::primary("Export this game…")).clicked() {
                        self.export = Some(tools::ExportState::from_base(base.clone()));
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
            }
            if ui.button("Refresh").clicked() {
                self.open_game(base);
            }
            if ui.button("Back").clicked() {
                self.game = None;
            }
        });
    }
}
