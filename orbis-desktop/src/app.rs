use crate::{
    operations::{self, Operation, Outcome, PackageInfo, human_size},
    worker::{Details, Job},
};
use eframe::egui;

mod view;
use orbis_cli::install::{InstallMode, InstallPlan, MergeStatus};
use std::{path::PathBuf, sync::atomic::Ordering, time::Duration};

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum Action {
    #[default]
    Extract,
    Update,
}

struct Notice {
    title: String,
    message: String,
    details: String,
    output: Option<PathBuf>,
    attention: bool,
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
    base: Option<PathBuf>,
    preview: Option<InstallPlan>,
    mode: InstallMode,
    merged: Option<MergeStatus>,
    job: Option<Job>,
    notice: Option<Notice>,
    warnings: Details,
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
        self.package = None;
        self.artwork = None;
        self.parent = None;
        self.base = None;
        self.merged = None;
        self.mode = InstallMode::Separate;
        self.folder_name.clear();
        self.resume = false;
        self.preview = None;
        self.start(path, Operation::Inspect);
    }

    fn start(&mut self, path: PathBuf, operation: Operation) {
        if self.job.is_some() {
            return;
        }
        self.notice = None;
        self.warnings = Details::default();
        match Job::start(path, operation, self.ctx.clone()) {
            Ok(job) => self.job = Some(job),
            Err(e) => self.local_error("Could not start the operation", e.to_string()),
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
                self.action = if info.update_supported {
                    Action::Update
                } else {
                    Action::Extract
                };
                self.folder_name = operations::suggested_folder(&info);
                self.parent = info.path.parent().map(PathBuf::from);
                self.package = Some(info);
            }
            Ok(Outcome::Preview(plan)) => self.preview = Some(plan),
            Ok(Outcome::GameState(state)) => {
                self.preview = None;
                self.merged = Some(state);
            }
            Ok(Outcome::Merged { state, output }) => {
                self.preview = None;
                self.notice = Some(Notice {
                    title: "Update merged".into(),
                    message: format!(
                        "{} is now at version {} in the game folder. Keep the adjacent backup folder. Choose Done to restore the base or manage this merge.",
                        state.title_id, state.update_version
                    ),
                    output: Some(output),
                    details: String::new(),
                    attention: false,
                });
                self.merged = Some(state);
            }
            Ok(Outcome::Finished {
                title,
                message,
                output,
                incomplete,
            }) => {
                self.merged = None;
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
            .set_title("Manage game folder")
            .pick_folder()
        {
            self.base = Some(base.clone());
            self.preview = None;
            self.merged = None;
            self.start(PathBuf::new(), Operation::CheckGame { base });
        }
    }

    fn choose_package(&mut self, frame: &eframe::Frame) {
        if let Some(path) = rfd::FileDialog::new()
            .set_parent(frame)
            .set_title("Open a PlayStation 4 package")
            .add_filter("PlayStation 4 package", &["pkg"])
            .pick_file()
        {
            self.open(path);
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
                    "Open one package at a time",
                    "Drop a single PKG file onto the window.".into(),
                );
            } else if let Some(file) = dropped.first() {
                self.open(file.path().to_owned());
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
