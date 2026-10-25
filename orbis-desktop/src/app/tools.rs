use super::view::{folder_picker, path_label, warning};
use super::*;
use crate::theme::{self, heading, muted};

pub(super) struct ExportState {
    base: Option<PathBuf>,
    parent: Option<PathBuf>,
    name: String,
    resume: bool,
    pub preview: Option<Box<orbis_cli::install::ExportPlan>>,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            base: None,
            parent: None,
            name: "merged-game".into(),
            resume: true,
            preview: None,
        }
    }
}

impl ExportState {
    fn output(&self) -> Option<PathBuf> {
        self.parent
            .as_ref()
            .filter(|_| operations::folder_name_valid(&self.name))
            .map(|p| p.join(&self.name))
    }
}

impl OrbisApp {
    pub(super) fn passcode_input_id() -> egui::Id {
        egui::Id::unique("package-passcode-input")
    }

    pub(super) fn reset_passcode_input(&mut self) {
        self.passcode_input = Default::default();
        let id = Self::passcode_input_id();
        if let Some(mut state) = egui::TextEdit::load_state(&self.ctx, id) {
            // egui shares undo/redo history with cloned widget state and Memory.
            // Clear that shared history before removing the stored editor state.
            state.clear_undoer();
        }
        self.ctx
            .data_mut(|data| data.remove::<egui::text_edit::TextEditState>(id));
    }

    pub(super) fn passcode_settings(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(if self.passcode.is_some() { "Package passcode · validated" } else { "Package passcode (optional)" })
            .show(ui, |ui| {
                ui.label(muted(ui, "Enter a known 32-character passcode to unlock encrypted metadata. It is kept only for this open package and is never saved in reports."));
                ui.add(egui::TextEdit::singleline(&mut *self.passcode_input).id(Self::passcode_input_id()).password(true).char_limit(32).hint_text("32-character package passcode"));
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.passcode_input.len() == 32, egui::Button::new("Validate passcode")).clicked() {
                        self.validate_passcode();
                    }
                    if ui.add_enabled(self.passcode.is_some(), egui::Button::new("Clear passcode")).clicked() {
                        self.clear_passcode();
                    }
                });
                if self.passcode.is_some() {
                    ui.label("Passcode accepted for this package. Details above show any entries that remain unavailable.");
                }
            });
        ui.add_space(6.0);
    }

    pub(super) fn clear_passcode(&mut self) {
        self.passcode = None;
        self.pending_passcode = None;
        self.reset_passcode_input();
        self.refresh_package();
    }

    pub(super) fn validate_passcode(&mut self) {
        let value = std::mem::take(&mut self.passcode_input);
        self.reset_passcode_input();
        match operations::Passcode::new(value) {
            Ok(passcode) => {
                self.pending_passcode = Some(Arc::new(passcode));
                self.refresh_package();
            }
            Err(error) => self.local_error("Invalid passcode", error.into()),
        }
    }

    fn refresh_package(&mut self) {
        if let Some(info) = &self.package {
            let path = info.path.clone();
            self.verification = None;
            self.preview = None;
            self.refreshing_package = true;
            self.start(path, Operation::Inspect);
        }
    }

    pub(super) fn verification_details(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let Some(report) = &self.verification else {
            return;
        };
        ui.add_space(10.0);
        ui.label(heading(report.title(), 15.0));
        ui.label(if report.metadata_only {
            "Scope: metadata only. Game data was not verified."
        } else {
            "Scope: full package integrity and readability."
        });
        ui.label(format!(
            "{} SHA-256 · {} HMAC · {} unavailable · {} failed",
            report.summary.digests,
            report.summary.macs,
            report.summary.unavailable,
            report.summary.failed
        ));
        if let Some(readability) = &report.readability {
            ui.label(format!(
                "{} files read ({}) · {} skipped",
                readability.files,
                human_size(readability.bytes),
                readability.skipped.len()
            ));
        }
        for (reason, count) in &report.unavailable_reasons {
            ui.label(muted(ui, format!("{reason}: {count}")));
        }
        ui.label(
            muted(
                ui,
                "Stored-data consistency does not establish publisher authenticity.",
            )
            .size(12.0),
        );
        if ui
            .add_enabled(
                self.job.is_none(),
                egui::Button::new("Save verification report…"),
            )
            .clicked()
            && let Some(destination) = rfd::FileDialog::new()
                .set_parent(frame)
                .set_title("Save verification summary to a new JSON file")
                .add_filter("JSON report", &["json"])
                .set_file_name("orbis-verification.json")
                .save_file()
        {
            if let Err(error) = report.save(&destination) {
                self.local_error(
                    "Could not save report",
                    format!("{error}. Choose a new filename; existing files are preserved."),
                );
            } else {
                self.notice = Some(Notice {
                    title: "Verification report saved".into(),
                    message: destination.display().to_string(),
                    details: String::new(),
                    output: destination.parent().map(PathBuf::from),
                    attention: false,
                });
            }
        }
    }

    pub(super) fn export_settings(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        ui.label(heading("Export merged game", 21.0));
        ui.label(muted(ui, "Copy an original base and its managed separate update into one independent folder. With no installed update, export the base alone."));
        ui.add_space(10.0);
        let state = self.export.as_mut().unwrap();
        let mut changed = folder_picker(ui, frame, "Original base game", &mut state.base);
        changed |= folder_picker(ui, frame, "Export destination", &mut state.parent);
        ui.horizontal(|ui| {
            ui.label("Folder name");
            changed |= ui.text_edit_singleline(&mut state.name).changed();
        });
        if !operations::folder_name_valid(&state.name) {
            warning(ui, "Use 1–100 letters, numbers, hyphens, or underscores.");
        }
        changed |= ui
            .checkbox(
                &mut state.resume,
                "Resume and retain completed files after interruption",
            )
            .changed();
        if changed {
            state.preview = None;
        }
        if let Some(output) = state.output() {
            path_label(ui, &output, "Output: ");
        }
        ui.label(muted(ui, "Close the emulator and keep the source files unchanged. An in-place merge must be restored before export. Existing conflicting files are never replaced.").size(12.0));
        if let Some(plan) = &state.preview {
            ui.separator();
            ui.label(heading(&plan.title_id, 16.0));
            ui.label(format!(
                "Base {} · Update {}",
                plan.base_version,
                plan.update_version.as_deref().unwrap_or("none")
            ));
            ui.label(format!(
                "{} files ({}) · {} from update",
                plan.files,
                human_size(plan.bytes),
                plan.update_files
            ));
            ui.label(format!(
                "Space reserve: {} · available {}",
                human_size(plan.filesystem.required_free_bytes),
                human_size(plan.filesystem.available_bytes)
            ));
            if plan.filesystem.available_bytes < plan.filesystem.required_free_bytes {
                warning(ui, "Not enough free space. Free space and check again.");
            }
            ui.label(muted(ui, "The space reserve includes staging and filesystem overhead. Resuming checks completed files and reserves space for missing files. The finished export is verified automatically.").size(12.0));
        }
        ui.add_space(10.0);
        if ui.button("Back").clicked() {
            self.export = None;
        }
    }

    pub(super) fn export_footer(&mut self, ui: &mut egui::Ui) {
        let state = self.export.as_mut().unwrap();
        let mut operation = None;
        ui.horizontal_wrapped(|ui| {
            if let Some(plan) = &state.preview {
                let enough = plan.filesystem.available_bytes >= plan.filesystem.required_free_bytes;
                if ui
                    .add_enabled(enough, theme::primary("Export and verify"))
                    .clicked()
                {
                    operation = Some(Operation::Export {
                        base: state.base.clone().unwrap(),
                        preview: state.preview.take().unwrap(),
                        resume: state.resume,
                    });
                }
                if ui.button("Check again").clicked() {
                    state.preview = None;
                }
            } else if ui
                .add_enabled(
                    state.base.is_some() && state.output().is_some(),
                    theme::primary("Check export"),
                )
                .clicked()
            {
                operation = Some(Operation::PreviewExport {
                    base: state.base.clone().unwrap(),
                    output: state.output().unwrap(),
                    resume: state.resume,
                });
            }
            ui.label(muted(ui, "The original game and update stay unchanged.").size(12.0));
        });
        if let Some(operation) = operation {
            self.start(PathBuf::new(), operation);
        }
    }
}
