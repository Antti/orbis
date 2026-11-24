use crate::{
    operations::{self, Operation, Outcome, OutputFolder, PackageInfo, human_size},
    worker::{Details, Job},
};
use eframe::egui;

mod game;
#[cfg(test)]
mod tests;
mod tools;
mod view;
use orbis_cli::install::{InstallMode, InstallPlan};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum Action {
    #[default]
    Extract,
    Update,
    Verify,
}

struct Notice {
    title: String,
    message: String,
    details: String,
    output: Option<OutputFolder>,
    attention: bool,
}

enum UpdateBase {
    Pick(Option<PathBuf>),
    Game(PathBuf),
}

impl Default for UpdateBase {
    fn default() -> Self {
        Self::Pick(None)
    }
}

impl UpdateBase {
    fn path(&self) -> Option<&PathBuf> {
        match self {
            Self::Pick(path) => path.as_ref(),
            Self::Game(path) => Some(path),
        }
    }
}

#[derive(Default)]
pub struct OrbisApp {
    ctx: egui::Context,
    package: Option<PackageInfo>,
    artwork: Option<egui::TextureHandle>,
    action: Action,
    parent: Option<PathBuf>,
    folder_name: String,
    resume: bool,
    base: UpdateBase,
    preview: Option<InstallPlan>,
    mode: InstallMode,
    game: Option<game::GameFolder>,
    job: Option<Job>,
    notice: Option<Notice>,
    warnings: Details,
    metadata_only: bool,
    verification: Option<Box<operations::VerificationReport>>,
    passcode_input: zeroize::Zeroizing<String>,
    passcode: Option<Arc<operations::Passcode>>,
    pending_passcode: Option<Arc<operations::Passcode>>,
    refreshing_package: bool,
    export: Option<tools::ExportState>,
    close_prompt: bool,
    close_after_job: bool,
}

impl OrbisApp {
    pub fn new(ctx: &egui::Context, path: Option<PathBuf>) -> Self {
        crate::theme::configure(ctx);
        let mut app = Self {
            ctx: ctx.clone(),
            ..Self::default()
        };
        if let Some(path) = path {
            app.open(path);
        }
        app
    }

    fn open(&mut self, path: PathBuf) {
        if path.is_dir() {
            self.open_game(path);
        } else {
            self.open_package(path, UpdateBase::default());
        }
    }

    fn reset_package(&mut self, base: UpdateBase) {
        self.passcode = None;
        self.pending_passcode = None;
        self.reset_passcode_input();
        self.verification = None;
        self.refreshing_package = false;
        self.export = None;
        self.package = None;
        self.artwork = None;
        self.parent = None;
        if !matches!((&self.base, &base), (UpdateBase::Game(old), UpdateBase::Game(new)) if old == new)
        {
            self.mode = InstallMode::Separate;
        }
        self.base = base;
        self.game = None;
        self.folder_name.clear();
        self.resume = false;
        self.preview = None;
    }

    fn open_package(&mut self, path: PathBuf, base: UpdateBase) {
        self.reset_package(base);
        self.inspect_package(path);
    }

    fn inspect_package(&mut self, path: PathBuf) {
        let update_base = match &self.base {
            UpdateBase::Game(base) => Some(base.clone()),
            UpdateBase::Pick(_) => None,
        };
        self.start(path, Operation::Inspect(update_base));
    }

    fn preview_update(&mut self) {
        if let Some(info) = self.package.as_ref().filter(|info| info.can_check_update())
            && let Some(base) = self.base.path().cloned()
        {
            self.preview = None;
            self.start(
                info.path.clone(),
                Operation::PreviewUpdate {
                    base,
                    mode: self.mode,
                },
            );
        }
    }

    fn start(&mut self, path: PathBuf, operation: Operation) {
        if self.job.is_some() {
            return;
        }
        if matches!(operation, Operation::Verify { .. }) {
            self.verification = None;
        }
        self.notice = None;
        self.warnings = Details::default();
        match Job::start(
            path,
            operation,
            self.ctx.clone(),
            self.pending_passcode
                .clone()
                .or_else(|| self.passcode.clone()),
        ) {
            Ok(job) => self.job = Some(job),
            Err(e) => {
                self.pending_passcode = None;
                self.refreshing_package = false;
                self.local_error("Could not start the operation", e.to_string());
            }
        }
        self.ctx.request_repaint();
    }

    fn local_error(&mut self, title: &str, details: String) {
        self.notice = Some(Notice {
            title: title.into(),
            message: String::new(),
            details,
            output: None,
            attention: true,
        });
    }

    fn poll(&mut self) {
        if !self.job.as_ref().is_some_and(Job::is_finished) {
            return;
        }
        let job = self.job.take().unwrap();
        let cancelled = job.progress.cancelled.load(Ordering::Relaxed);
        self.close_prompt = false;
        let label = job.label;
        let help = job.help;
        self.warnings = job.progress.snapshot();
        match job.finish() {
            Ok(Outcome::Inspected(mut info)) => {
                self.artwork = info.artwork.take().map(|image| {
                    self.ctx
                        .load_texture("package-artwork", image, egui::TextureOptions::LINEAR)
                });
                if let Some(passcode) = self.pending_passcode.take() {
                    self.passcode = Some(passcode);
                }
                let from_game = matches!(self.base, UpdateBase::Game(_));
                if from_game {
                    self.action = Action::Update;
                } else if !self.refreshing_package {
                    self.action = if info.update_error.is_none() {
                        Action::Update
                    } else {
                        Action::Extract
                    };
                    self.folder_name = operations::suggested_folder(&info);
                    self.parent = info.path.parent().map(PathBuf::from);
                } else if self.action == Action::Update && info.update_error.is_some() {
                    self.action = Action::Extract;
                }
                self.refreshing_package = false;
                self.package = Some(info);
                if from_game && !cancelled && !self.close_after_job {
                    self.preview_update();
                }
            }
            Ok(Outcome::Verified(report)) => {
                self.notice = Some(Notice {
                    title: report.title().into(),
                    message: String::new(),
                    details: report.error.clone().unwrap_or_default(),
                    output: None,
                    attention: report.status != "complete",
                });
                self.verification = Some(report);
            }
            Ok(Outcome::ExportPreview(plan)) => {
                if let Some(export) = &mut self.export {
                    export.preview = Some(plan);
                }
            }
            Ok(Outcome::Preview(plan)) => self.preview = Some(plan),
            Ok(Outcome::GameStatus {
                base,
                status,
                rollback,
            }) => {
                self.preview = None;
                self.game = Some(game::GameFolder::new(base, Some(status), rollback));
            }
            Ok(Outcome::GameChanged {
                base,
                title,
                message,
            }) => {
                self.preview = None;
                self.game = Some(game::GameFolder::new(base.clone(), None, None));
                self.notice = Some(Notice {
                    title: title.into(),
                    message,
                    output: Some(OutputFolder::Directory(base)),
                    details: String::new(),
                    attention: false,
                });
            }
            Ok(Outcome::Finished {
                title,
                message,
                output,
                incomplete,
            }) => {
                self.preview = None;
                self.notice = Some(Notice {
                    title,
                    message,
                    output: Some(output),
                    details: String::new(),
                    attention: incomplete,
                });
            }
            Err(failure) => {
                self.pending_passcode = None;
                self.refreshing_package = false;
                if let Some(export) = &mut self.export {
                    export.preview = None;
                }
                self.preview = None;
                self.notice = Some(Notice {
                    title: if failure.cancelled {
                        "Operation cancelled".into()
                    } else {
                        format!("{label} failed")
                    },
                    message: help.into(),
                    details: failure.details,
                    output: None,
                    attention: true,
                });
            }
        }
    }

    fn choose_game(&mut self, frame: &eframe::Frame) {
        if let Some(base) = rfd::FileDialog::new()
            .set_parent(frame)
            .set_title("Open extracted base game folder")
            .pick_folder()
        {
            self.open_game(base);
        }
    }

    fn choose_package(&mut self, frame: &eframe::Frame, base: UpdateBase) {
        if let Some(path) = rfd::FileDialog::new()
            .set_parent(frame)
            .set_title(match base {
                UpdateBase::Game(_) => "Choose an update PKG for this game",
                UpdateBase::Pick(_) => "Open a PlayStation 4 package",
            })
            .add_filter("PlayStation 4 package", &["pkg"])
            .pick_file()
        {
            self.open_package(path, base);
        }
    }
}

impl eframe::App for OrbisApp {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        if self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_prompt = !self.close_after_job;
            }
        } else if self.close_after_job {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.job.is_none() && !self.close_after_job {
            // Consume drops during painting, not logic: logic can also run with
            // the previous frame's input while the window is hidden.
            let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
            if dropped.len() > 1 {
                self.local_error(
                    "Open one item at a time",
                    "Drop a PKG file or an extracted game folder onto the window.".into(),
                );
            } else if let Some(file) = dropped.first() {
                let path = file.path().to_owned();
                if let UpdateBase::Game(base) = &self.base
                    && self.game.is_none()
                    && self.export.is_none()
                    && !path.is_dir()
                {
                    self.open_package(path, UpdateBase::Game(base.clone()));
                } else {
                    self.open(path);
                }
            }
        }
        if self.close_prompt {
            egui::Modal::new(egui::Id::unique("close-operation")).show(&ctx, |ui| {
                ui.heading("An operation is still running");
                ui.label(
                    "Orbis can cancel and close after the current operation has stopped safely.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Keep working").clicked() {
                        self.close_prompt = false;
                    }
                    if ui.button("Cancel and close").clicked() {
                        if let Some(job) = &self.job {
                            job.cancel();
                        }
                        self.close_after_job = true;
                        self.close_prompt = false;
                    }
                });
            });
        }
        self.render(ui, frame);
        if self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
