use super::*;
use crate::theme::{self, heading, muted};
use egui::{Color32, RichText};
use orbis_cli::install::ChangeKind;
use std::path::Path;

impl OrbisApp {
    pub(super) fn render(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        egui::Panel::top("toolbar")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(egui::Margin::symmetric(20, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::package_mark(ui, 26.0);
                    ui.label(heading("Orbis", 18.0));
                    {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(self.job.is_none(), egui::Button::new("Export game…"))
                                .clicked()
                            {
                                self.export = Some(super::tools::ExportState::default());
                                self.merged = None;
                                self.notice = None;
                            }
                            if ui
                                .add_enabled(self.job.is_none(), egui::Button::new("Manage game…"))
                                .clicked()
                            {
                                self.choose_game(frame);
                            }
                            if ui
                                .add_enabled(self.job.is_none(), egui::Button::new("Open PKG…"))
                                .clicked()
                            {
                                self.choose_package(frame);
                            }
                        });
                    }
                });
            });
        if self.export.is_some()
            || self.package.is_some()
            || self.job.is_some()
            || self.notice.is_some()
            || self.merged.is_some()
        {
            egui::Panel::bottom("operation-bar")
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(ui.visuals().panel_fill)
                        .inner_margin(egui::Margin::symmetric(20, 12)),
                )
                .show(ui, |ui| self.footer(ui));
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.export.is_some() {
                            if self.notice.is_none() {
                                ui.add_enabled_ui(
                                    self.job.is_none() && !self.close_after_job,
                                    |ui| self.export_settings(ui, frame),
                                );
                            }
                        } else if self.merged.is_some() {
                            self.merge_details(ui);
                        } else if self.package.is_none() {
                            self.empty_view(ui, frame);
                        } else {
                            self.package_summary(ui);
                            if self.notice.is_none() {
                                ui.add_enabled_ui(
                                    self.job.is_none() && !self.close_after_job,
                                    |ui| {
                                        self.passcode_settings(ui);
                                        self.settings(ui, frame);
                                    },
                                );
                            }
                        }
                        self.result_details(ui);
                        if self.export.is_none() && self.package.is_some() {
                            self.verification_details(ui, frame);
                        }
                    });
            });
    }

    fn empty_view(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        ui.add_space((ui.available_height() * 0.18).max(12.0));
        ui.vertical_centered(|ui| {
            theme::package_mark(ui, 64.0);
            ui.add_space(12.0);
            ui.label(heading("Open a PlayStation 4 package", 21.0));
            ui.add_space(2.0);
            ui.label(muted(
                ui,
                "Drop a .pkg file here, or choose one to get started.",
            ));
            ui.add_space(16.0);
            if ui
                .add_enabled(self.job.is_none(), theme::primary("Open PKG…"))
                .clicked()
            {
                self.choose_package(frame);
            }
            ui.add_space(20.0);
            ui.label(muted(ui, "Extract files  ·  Verify packages  ·  Install updates").size(13.0));
        });
    }

    fn package_summary(&self, ui: &mut egui::Ui) {
        let info = self.package.as_ref().unwrap();
        ui.horizontal(|ui| {
            if let Some(texture) = &self.artwork {
                ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(egui::vec2(64.0, 64.0))
                        .corner_radius(9),
                );
            } else {
                theme::package_mark(ui, 64.0);
            }
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.add(egui::Label::new(heading(&info.title, 21.0)).truncate())
                    .on_hover_text(&info.title);
                ui.horizontal(|ui| {
                    egui::Frame::new()
                        .fill(ui.visuals().selection.bg_fill)
                        .corner_radius(4)
                        .inner_margin(egui::Margin::symmetric(7, 3))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(&info.kind)
                                    .size(12.0)
                                    .color(ui.visuals().selection.stroke.color),
                            );
                        });
                    ui.label(muted(ui, &info.title_id).size(13.0));
                });
                let filename = info.path.file_name().unwrap_or_default().to_string_lossy();
                ui.add(egui::Label::new(muted(ui, filename).size(12.0)).truncate())
                    .on_hover_text(info.path.display().to_string());
            });
        });
        ui.add_space(6.0);
        ui.columns(3, |columns| {
            metric(
                &mut columns[0],
                "App version",
                info.version.as_deref().unwrap_or("Unavailable"),
            );
            metric(&mut columns[1], "PKG size", &human_size(info.size));
            metric(
                &mut columns[2],
                "Minimum system",
                info.required_system.as_deref().unwrap_or("Unavailable"),
            );
        });
        egui::CollapsingHeader::new("Package details").show(ui, |ui| {
            detail(ui, "Content ID", &info.content_id);
            detail(ui, "Package version", info.package_version.as_deref().unwrap_or("Unavailable"));
            detail(ui, "Region from ID", info.region.unwrap_or("Unknown"));
            detail(ui, "Metadata entries", &info.metadata_entries.to_string());
            detail(ui, "Integrity", self.verification.as_ref().map_or("Not checked", |r| r.title()));
            detail(ui, "Package file", &info.path.display().to_string());
            ui.label(muted(ui, "Region is inferred from the content ID. The system version is declared by the package; neither establishes emulator compatibility.").size(12.0));
            if !info.update_supported { ui.label(muted(ui, format!("Update installation: {}", info.update_note)).size(12.0)); }
        });
        if info.missing_keys > 0 {
            warning(
                ui,
                &format!(
                    "{} encrypted metadata {} cannot be read. Extraction may be incomplete.",
                    info.missing_keys,
                    if info.missing_keys == 1 {
                        "entry"
                    } else {
                        "entries"
                    }
                ),
            );
        }
        for message in &info.warnings {
            warning(ui, message);
        }
        ui.add_space(5.0);
        ui.separator();
        ui.add_space(6.0);
    }

    fn settings(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let info = self.package.as_ref().unwrap();
        let old_action = self.action;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.action,
                Action::Extract,
                heading("Extract files", 14.0),
            );
            ui.add_enabled_ui(info.update_supported, |ui| {
                ui.selectable_value(
                    &mut self.action,
                    Action::Update,
                    heading("Install update", 14.0),
                )
                .on_disabled_hover_text(&info.update_note);
            });
            ui.selectable_value(
                &mut self.action,
                Action::Verify,
                heading("Verify package", 14.0),
            );
        });
        if old_action != self.action {
            self.preview = None;
        }
        ui.add_space(8.0);
        match self.action {
            Action::Verify => {
                ui.radio_value(
                    &mut self.metadata_only,
                    false,
                    "Full integrity and readability",
                );
                ui.radio_value(&mut self.metadata_only, true, "Metadata only (quicker)");
                ui.label(muted(ui, "Full verification reads the package and checks stored hashes, supported HMACs, and file readability. Metadata only excludes game data. Missing checks produce an incomplete result.").size(13.0));
            }
            Action::Extract => {
                if folder_picker(ui, frame, "Destination", &mut self.parent) {
                    self.notice = None;
                }
                ui.horizontal(|ui| {
                    ui.label(muted(ui, "Folder name"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.folder_name)
                            .desired_width(ui.available_width()),
                    );
                });
                let valid = operations::folder_name_valid(&self.folder_name);
                if !valid {
                    warning(ui, "Use 1–100 letters, numbers, hyphens, or underscores.");
                }
                if let Some(parent) = self.parent.as_ref().filter(|_| valid) {
                    path_label(ui, &parent.join(&self.folder_name), "Output: ");
                }
                ui.checkbox(&mut self.resume, "Resume an unfinished extraction")
                    .on_hover_text(
                        "Existing files are checked before reuse. Conflicting files are preserved.",
                    );
            }
            Action::Update => {
                let old_mode = self.mode;
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut self.mode,
                        InstallMode::Separate,
                        "Keep update separate",
                    );
                    ui.selectable_value(
                        &mut self.mode,
                        InstallMode::Merge,
                        "Merge into game folder",
                    );
                });
                if self.mode != old_mode {
                    self.preview = None;
                }
                ui.add_space(5.0);
                if folder_picker(ui, frame, "Extracted base game", &mut self.base) {
                    self.preview = None;
                    self.merged = None;
                }
                if let Some(plan) = &self.preview {
                    ui.add_space(4.0);
                    ui.columns(2, |columns| {
                        metric(&mut columns[0], "Installed version", &plan.current_version);
                        metric(&mut columns[1], "Update version", &plan.update_version);
                    });
                    let count = |kind| plan.files.iter().filter(|file| file.change == kind).count();
                    let changes = if plan.mode == InstallMode::Merge {
                        format!(
                            "{} new  ·  {} replaced  ·  {} of originals retained",
                            count(ChangeKind::Add),
                            count(ChangeKind::Replace),
                            human_size(plan.backup_bytes)
                        )
                    } else {
                        format!(
                            "{} new  ·  {} replaced  ·  {} removed from update",
                            count(ChangeKind::Add),
                            count(ChangeKind::Replace),
                            count(ChangeKind::RemoveFromUpdate)
                        )
                    };
                    ui.label(muted(ui, changes).size(13.0));
                    ui.label(
                        muted(
                            ui,
                            format!(
                                "Space: about {} needed · {} available",
                                human_size(plan.required_free_bytes),
                                human_size(plan.available_bytes)
                            ),
                        )
                        .size(13.0),
                    );
                    path_label(ui, &plan.destination, "Destination: ");
                    if plan.available_bytes < plan.required_free_bytes {
                        warning(
                            ui,
                            "Not enough free space. Free space on this drive, then check again.",
                        );
                    }
                    let note = if plan.mode == InstallMode::Merge {
                        "Replaces files in this game folder. Originals are backed up beside it. Restore the base before applying another update."
                    } else {
                        "Base files stay unchanged. One previous installation is kept for rollback."
                    };
                    ui.label(muted(ui, note).size(12.0));
                } else {
                    ui.label(muted(ui, if self.mode == InstallMode::Merge {
                        "Choose an original base game. Update files will replace game files, with backups for Restore base. Close the emulator first."
                    } else {
                        "Choose the game folder containing sce_sys. Orbis will check its identity, version, and available space."
                    }).size(13.0));
                }
            }
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            let details = job.progress.snapshot();
            let cancelling = job.progress.cancelled.load(Ordering::Relaxed);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(heading(
                    if cancelling {
                        "Cancelling safely…"
                    } else {
                        &details.phase
                    },
                    14.0,
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(!cancelling, egui::Button::new("Cancel"))
                        .clicked()
                    {
                        job.cancel();
                    }
                });
            });
            let total = job.progress.total.load(Ordering::Relaxed);
            let done = job.progress.processed.load(Ordering::Relaxed);
            if total > 0 {
                ui.add(
                    egui::ProgressBar::new((done as f64 / total as f64).clamp(0.0, 1.0) as f32)
                        .fill(theme::ACCENT)
                        .text(format!("{} / {}", human_size(done), human_size(total))),
                );
            }
            if !details.file.is_empty() {
                ui.add(egui::Label::new(muted(ui, &details.file).size(12.0)).truncate())
                    .on_hover_text(&details.file);
            }
            return;
        }
        if let Some(notice) = &self.notice {
            let mut done = false;
            let mut reveal_error = None;
            ui.horizontal(|ui| {
                ui.label(heading(&notice.title, 14.0).color(if notice.attention {
                    theme::attention(ui)
                } else {
                    ui.visuals().text_color()
                }));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(output) = &notice.output
                        && ui.add(theme::primary("Show in folder")).clicked()
                        && let Err(error) = open::that(output)
                    {
                        reveal_error = Some(error.to_string());
                    }
                    done = ui.button("Done").clicked();
                });
            });
            if done {
                self.notice = None;
                self.warnings = Details::default();
            }
            if let Some(error) = reveal_error {
                self.local_error("Could not open the folder", error);
            }
            return;
        }
        if self.export.is_some() {
            self.export_footer(ui);
            return;
        }
        if let Some(state) = &self.merged {
            let pending = state.pending_recovery;
            ui.horizontal(|ui| {
                ui.label(muted(ui, "Close the emulator before restoring.").size(12.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(theme::primary(if pending {
                            "Recover base"
                        } else {
                            "Restore base"
                        }))
                        .clicked()
                    {
                        let base = self.base.clone().unwrap();
                        self.start(
                            PathBuf::new(),
                            if pending {
                                Operation::RecoverBase { base }
                            } else {
                                Operation::RestoreBase { base }
                            },
                        );
                    }
                });
            });
            return;
        }
        let Some(package) = &self.package else {
            return;
        };
        let path = package.path.clone();
        ui.horizontal(|ui| {
            let caption = match self.action {
                Action::Verify => "Verification does not write package data.",
                Action::Extract => "Existing files are preserved.",
                Action::Update if self.preview.is_some() => "Close the emulator before installing.",
                Action::Update => "Check compatibility before installing.",
            };
            ui.label(muted(ui, caption).size(12.0));
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match self.action {
                    Action::Verify => {
                        if ui.add(theme::primary("Verify package")).clicked() {
                            self.start(
                                path,
                                Operation::Verify {
                                    metadata_only: self.metadata_only,
                                },
                            );
                        }
                    }
                    Action::Extract => {
                        let output = self
                            .parent
                            .as_ref()
                            .filter(|_| operations::folder_name_valid(&self.folder_name))
                            .map(|parent| parent.join(&self.folder_name));
                        if ui
                            .add_enabled(output.is_some(), theme::primary("Extract files"))
                            .clicked()
                        {
                            self.start(
                                path,
                                Operation::Extract {
                                    output: output.unwrap(),
                                    resume: self.resume,
                                },
                            );
                        }
                    }
                    Action::Update => {
                        if let Some(plan) = &self.preview {
                            let enough = plan.available_bytes >= plan.required_free_bytes;
                            if enough
                                && ui
                                    .add(theme::primary(if plan.mode == InstallMode::Merge {
                                        "Merge update"
                                    } else {
                                        "Install update"
                                    }))
                                    .clicked()
                            {
                                let preview = Box::new(self.preview.take().unwrap());
                                self.start(
                                    path,
                                    Operation::InstallUpdate {
                                        base: self.base.clone().unwrap(),
                                        preview,
                                    },
                                );
                            } else if !enough && ui.add(theme::primary("Check again")).clicked() {
                                self.preview = None;
                                self.start(
                                    path,
                                    Operation::PreviewUpdate {
                                        mode: self.mode,
                                        base: self.base.clone().unwrap(),
                                    },
                                );
                            }
                        } else if ui
                            .add_enabled(self.base.is_some(), theme::primary("Check update"))
                            .clicked()
                        {
                            self.start(
                                path,
                                Operation::PreviewUpdate {
                                    mode: self.mode,
                                    base: self.base.clone().unwrap(),
                                },
                            );
                        }
                    }
                },
            );
        });
    }

    fn merge_details(&self, ui: &mut egui::Ui) {
        let state = self.merged.as_ref().unwrap();
        ui.add_space(12.0);
        ui.label(heading(
            if state.pending_recovery {
                "Merge needs recovery"
            } else {
                "Merged game"
            },
            21.0,
        ));
        ui.label(muted(ui, &state.title_id));
        ui.add_space(12.0);
        ui.columns(2, |columns| {
            metric(&mut columns[0], "Original base", &state.base_version);
            metric(&mut columns[1], "Merged update", &state.update_version);
        });
        if let Some(base) = &self.base {
            path_label(ui, base, "Game folder: ");
        }
        ui.add_space(12.0);
        ui.label(if state.pending_recovery {
            "The merge or restoration was interrupted. Recover base restores the original game files before you use this folder."
        } else {
            "Update files are installed directly in this game folder. Restore base removes the merged update and puts the original files back. Restore before installing a different update."
        });
        ui.add_space(8.0);
        ui.label(muted(ui, "Keep the adjacent .<game>.orbis-install backup folder with your game. Orbis checks changed files and backups before restoring.").size(13.0));
    }

    fn result_details(&self, ui: &mut egui::Ui) {
        if let Some(notice) = &self.notice {
            ui.add_space(8.0);
            ui.label(&notice.message);
            if let Some(path) = &notice.output {
                path_label(ui, path, "Output: ");
            }
            if !notice.details.is_empty() {
                egui::CollapsingHeader::new("Error details")
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.label(&notice.details);
                    });
            }
        }
        let warnings = self
            .job
            .as_ref()
            .map_or_else(|| self.warnings.clone(), |job| job.progress.snapshot());
        if warnings.warning_count > 0 {
            egui::CollapsingHeader::new(format!("Warnings ({})", warnings.warning_count))
                .default_open(true)
                .show(ui, |ui| {
                    for warning in &warnings.warnings {
                        ui.label(warning);
                    }
                    if warnings.warning_count > warnings.warnings.len() {
                        ui.label(muted(ui, "Showing the first 100 warnings."));
                    }
                });
        }
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(muted(ui, label).size(12.0));
    ui.label(heading(value, 15.0));
}

fn detail(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(120.0, 18.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(120.0);
                ui.label(muted(ui, label).size(13.0));
            },
        );
        ui.add(egui::Label::new(RichText::new(value).size(13.0)).wrap())
            .context_menu(|ui| {
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(value.into());
                    ui.close();
                }
            });
    });
}

pub(super) fn path_label(ui: &mut egui::Ui, path: &Path, prefix: &str) {
    let full = format!("{prefix}{}", path.display());
    ui.add(egui::Label::new(muted(ui, &full).size(12.0)).truncate())
        .on_hover_text(&full);
}

pub(super) fn folder_picker(
    ui: &mut egui::Ui,
    frame: &eframe::Frame,
    label: &str,
    path: &mut Option<PathBuf>,
) -> bool {
    ui.label(heading(label, 13.0));
    let mut changed = false;
    ui.horizontal(|ui| {
        let text = path
            .as_ref()
            .map_or_else(|| "Choose a folder".into(), |p| p.display().to_string());
        let width = (ui.available_width() - 86.0).max(100.0);
        let response = ui.add_sized(
            [width, 32.0],
            egui::Button::new(RichText::new(&text).size(13.0)).truncate(),
        );
        let clicked = response.on_hover_text(&text).clicked() | ui.button("Browse…").clicked();
        if clicked
            && let Some(folder) = rfd::FileDialog::new()
                .set_parent(frame)
                .set_title(label)
                .pick_folder()
        {
            *path = Some(folder);
            changed = true;
            // A native dialog returns after this frame's widgets were painted.
            ui.ctx().request_repaint();
        }
    });
    changed
}

pub(super) fn warning(ui: &mut egui::Ui, message: &str) {
    let fill = if ui.visuals().dark_mode {
        Color32::from_rgb(58, 44, 28)
    } else {
        Color32::from_rgb(255, 245, 227)
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(6)
        .inner_margin(9)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                RichText::new(message)
                    .size(13.0)
                    .color(theme::attention(ui)),
            );
        });
}
