use crate::operations::{self, Failure, Operation, Outcome};
use orbis_cli::ExtractProgress;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

#[derive(Default, Clone)]
pub struct Details {
    pub phase: String,
    pub file: String,
    pub warnings: Vec<String>,
    pub warning_count: usize,
}

#[derive(Default)]
pub struct Progress {
    pub details: Mutex<Details>,
    pub total: AtomicU64,
    pub processed: AtomicU64,
    pub cancelled: AtomicBool,
}

impl Progress {
    pub fn snapshot(&self) -> Details {
        self.details
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn update(&self, f: impl FnOnce(&mut Details)) {
        f(&mut self.details.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

impl ExtractProgress for Progress {
    fn phase(&self, name: &str) {
        self.update(|s| {
            s.phase = name.into();
            s.file.clear();
        });
    }
    fn bytes_total(&self, bytes: u64) {
        self.processed.store(0, Ordering::Relaxed);
        self.total.store(bytes, Ordering::Relaxed);
    }
    fn bytes_processed(&self, bytes: u64) {
        self.processed.fetch_add(bytes, Ordering::Relaxed);
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    fn warning(&self, message: &str) {
        self.update(|s| {
            s.warning_count += 1;
            // Progress is coalesced, not queued. Large packages cannot flood the UI.
            if s.warnings.len() < 100 {
                s.warnings.push(message.chars().take(4096).collect());
            }
        });
    }
    fn entry_skipped(&self, path: &Path, reason: &str) {
        self.warning(&format!("{}: {reason}", path.display()));
    }
    fn entry_start(&self, path: &Path, _: usize, _: usize) {
        self.pfs_file(path, 0);
    }
    fn pfs_file(&self, path: &Path, _: u64) {
        self.update(|s| s.file = path.to_string_lossy().chars().take(1024).collect());
    }
}

pub struct Job {
    pub label: &'static str,
    pub help: &'static str,
    pub progress: Arc<Progress>,
    thread: JoinHandle<Result<Outcome, Failure>>,
}

impl Job {
    pub fn start(
        path: PathBuf,
        operation: Operation,
        ctx: eframe::egui::Context,
        passcode: Option<Arc<operations::Passcode>>,
    ) -> std::io::Result<Self> {
        let label = operation.label();
        let help = operation.failure_help();
        let progress = Arc::new(Progress::default());
        progress.phase(label);
        let worker_progress = progress.clone();
        let thread = thread::Builder::new()
            .name("orbis-operation".into())
            .spawn(move || {
                let result = operations::run_with_passcode(
                    &path,
                    operation,
                    passcode.as_deref(),
                    worker_progress.as_ref(),
                );
                // A modal native picker can outlast the frame's repaint timer.
                // Wake the event loop explicitly when background work finishes.
                ctx.request_repaint();
                result
            })?;
        Ok(Self {
            label,
            help,
            progress,
            thread,
        })
    }
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }
    pub fn cancel(&self) {
        self.progress.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn finish(self) -> Result<Outcome, Failure> {
        self.thread.join().unwrap_or_else(|_| Err(Failure { cancelled: false, details: "The operation stopped unexpectedly. Completed extraction files may remain. If an update was being installed, check its status with the Orbis CLI before retrying.".into() }))
    }
}
