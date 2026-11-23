use super::view::{folder_picker, path_label, warning};
use super::*;
use crate::theme::{self, heading, muted};

pub(super) struct ExportState {
    base: Option<PathBuf>,
    parent: Option<PathBuf>,
    name: String,
    resume: bool,
    pub preview: Option<Box<orbis_cli::install::ExportPreview>>,
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
    pub(super) fn from_base(base: PathBuf) -> Self {
        Self {
            base: Some(base),
            ..Self::default()
        }
    }

    fn output(&self) -> Option<PathBuf> {
        self.parent
            .as_ref()
            .filter(|_| operations::folder_name_valid(&self.name))
            .map(|p| p.join(&self.name))
    }

    fn check_export(&mut self) -> Option<Operation> {
        self.preview = None;
        Some(Operation::PreviewExport {
            base: self.base.clone()?,
            output: self.output()?,
            resume: self.resume,
        })
    }

    fn change_destination(&mut self, parent: PathBuf) -> Option<Operation> {
        self.parent = Some(parent);
        self.check_export()
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
                    output: destination
                        .parent()
                        .map(|p| OutputFolder::Directory(p.to_owned())),
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
        if let Some(preview) = &state.preview {
            let plan = preview.plan();
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
            if let Some(work) = preview.resume_plan() {
                ui.label(format!(
                    "{} files reusable · {} files to copy ({})",
                    work.reusable_files,
                    work.remaining_files,
                    human_size(work.remaining_bytes)
                ));
            }
            ui.label(muted(ui, "The space reserve includes staging and filesystem overhead. Resuming checks completed files and reserves space for missing files. The finished export is verified automatically.").size(12.0));
        }
        ui.add_space(10.0);
        if ui.button("Back").clicked() {
            self.export = None;
        }
    }

    pub(super) fn export_footer(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let state = self.export.as_mut().unwrap();
        let mut operation = None;
        let shortfall = state
            .preview
            .as_ref()
            .map_or(0, |preview| preview.plan().filesystem.shortfall_bytes());
        if shortfall > 0 {
            warning(
                ui,
                &format!(
                    "This destination needs {} more free space.",
                    human_size(shortfall)
                ),
            );
        }
        ui.horizontal_wrapped(|ui| {
            if shortfall > 0 {
                if ui.add(theme::primary("Choose another drive…")).clicked()
                    && let Some(parent) = rfd::FileDialog::new()
                        .set_parent(frame)
                        .set_title("Choose a folder for the export")
                        .pick_folder()
                {
                    operation = state.change_destination(parent);
                }
            } else if state.preview.is_some()
                && ui.add(theme::primary("Export and verify")).clicked()
            {
                operation = state.preview.take().map(Operation::Export);
            }
            let check_button = if state.preview.is_some() {
                egui::Button::new("Check again")
            } else {
                theme::primary("Check export")
            };
            if operation.is_none()
                && ui
                    .add_enabled(
                        state.base.is_some() && state.output().is_some(),
                        check_button,
                    )
                    .clicked()
            {
                operation = state.check_export();
            }
            ui.label(muted(ui, "The original game and update stay unchanged.").size(12.0));
        });
        if let Some(operation) = operation {
            self.start(PathBuf::new(), operation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{DroppedPath, click, finish, frame};
    use super::*;
    use orbis_cli::{SilentProgress, install::ExportPreview};
    use std::fs;

    #[test]
    fn open_game_and_export_the_installed_separate_update() {
        let temp = tempfile::tempdir().unwrap();
        let base = crate::common::base_game(temp.path());
        let mut app = OrbisApp::default();
        app.open(base.clone());
        finish(&mut app);
        let view = frame(&mut app, Default::default());
        assert!(
            view.nodes
                .iter()
                .any(|(_, node)| node.value() == Some("Base game only"))
        );
        assert!(!temp.path().join(".CUSA00000.orbis-install").exists());

        let path = temp.path().join("update.pkg");
        fs::write(&path, crate::common::update("01.12", "base.txt", b"PATCH")).unwrap();
        app.open(path);
        finish(&mut app);
        assert!(app.game.is_none());
        app.base = UpdateBase::Pick(Some(base.clone()));
        click(&mut app, "Check update");
        finish(&mut app);
        click(&mut app, "Install update");
        finish(&mut app);
        assert_eq!(app.notice.as_ref().unwrap().title, "Update installed");

        frame(
            &mut app,
            egui::RawInput {
                dropped_files: vec![Arc::new(DroppedPath(base.clone()))],
                ..Default::default()
            },
        );
        finish(&mut app);
        let view = frame(&mut app, Default::default());
        for label in [
            "Separate update",
            "01.00",
            "Active version",
            "01.12",
            "Rollback target: Base 01.00",
        ] {
            assert!(
                view.nodes
                    .iter()
                    .any(|(_, node)| node.value() == Some(label)),
                "missing {label}"
            );
        }
        click(&mut app, "Export this game…");
        let export = app.export.as_mut().unwrap();
        assert_eq!(export.base.as_ref(), Some(&base));
        assert!(export.parent.is_none() && export.preview.is_none());
        export.parent = Some(temp.path().to_owned());
        click(&mut app, "Check export");
        finish(&mut app);
        click(&mut app, "Export and verify");
        finish(&mut app);
        assert_eq!(
            app.notice.as_ref().unwrap().title,
            "Export complete and verified"
        );
        assert_eq!(
            fs::read(temp.path().join("merged-game/base.txt")).unwrap(),
            b"PATCH"
        );
        assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
        click(&mut app, "Done");
        click(&mut app, "Back");
        assert!(app.export.is_none());
        assert_eq!(app.game.as_ref().unwrap().base, base);
    }

    #[test]
    fn choosing_another_drive_rechecks_the_new_parent_with_the_same_settings() {
        let temp = tempfile::tempdir().unwrap();
        let base = crate::common::base_game(temp.path());
        let next = temp.path().join("other-drive");
        fs::create_dir(&next).unwrap();
        for resume in [true, false] {
            let name = "my-export";
            let preview =
                ExportPreview::new(&base, &temp.path().join(name), resume, SilentProgress).unwrap();
            let mut state = ExportState {
                base: Some(base.clone()),
                parent: Some(temp.path().to_owned()),
                name: name.into(),
                resume,
                preview: Some(Box::new(preview)),
            };
            let Some(Operation::PreviewExport {
                base: selected,
                output,
                resume: selected_resume,
            }) = state.change_destination(next.clone())
            else {
                panic!("choosing a destination must start a new check");
            };
            assert_eq!(selected, base);
            assert_eq!(output, next.join(name));
            assert_eq!(selected_resume, resume);
            assert!(state.preview.is_none());
        }
    }

    #[test]
    fn check_again_starts_a_new_preview_with_one_click() {
        let temp = tempfile::tempdir().unwrap();
        let base = crate::common::base_game(temp.path());
        let preview = ExportPreview::new(
            &base,
            &temp.path().join("merged-game"),
            true,
            SilentProgress,
        )
        .unwrap();
        let mut app = OrbisApp {
            export: Some(ExportState {
                base: Some(base),
                parent: Some(temp.path().to_owned()),
                preview: Some(Box::new(preview)),
                ..ExportState::default()
            }),
            ..OrbisApp::default()
        };
        click(&mut app, "Check again");
        let job = app
            .job
            .take()
            .expect("Check again should dispatch the preview");
        assert!(matches!(job.finish().unwrap(), Outcome::ExportPreview(_)));
    }
}
