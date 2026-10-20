use std::path::Path;

/// Trait for receiving extraction progress updates.
///
/// All methods take `&self` (not `&mut self`) so the progress reporter can be
/// shared across threads during parallel extraction.
pub trait ExtractProgress: Send + Sync {
    /// A new inspection, extraction, or verification phase.
    fn phase(&self, _name: &str) {}

    /// Total bytes to process, including metadata and uncompressed file data.
    fn bytes_total(&self, _bytes: u64) {}

    /// Bytes processed since the preceding update.
    fn bytes_processed(&self, _bytes: u64) {}

    /// Cooperative cancellation, checked before writes and between chunks.
    fn is_cancelled(&self) -> bool {
        false
    }

    /// Called when starting to extract a PKG entry.
    fn entry_start(&self, _path: &Path, _current: usize, _total: usize) {}

    /// Called when an entry is skipped (e.g., cannot be decrypted).
    fn entry_skipped(&self, _path: &Path, _reason: &str) {}

    /// Called when all PKG entries have been extracted.
    fn entries_completed(&self, _extracted: usize, _skipped: usize) {}

    /// Called when starting PFS extraction.
    fn pfs_start(&self, _total_items: usize) {}

    /// Called when creating a directory from PFS.
    fn pfs_directory(&self, _path: &Path) {}

    /// Called when starting to extract a file from PFS.
    fn pfs_file(&self, _path: &Path, _size: u64) {}

    /// Called when a PFS file has been fully extracted.
    fn pfs_file_completed(&self, _written: u64) {}

    /// Called when PFS extraction is complete.
    fn pfs_completed(&self) {}
}

impl<P: ExtractProgress + ?Sized> ExtractProgress for &P {
    fn phase(&self, name: &str) {
        (**self).phase(name);
    }
    fn bytes_total(&self, bytes: u64) {
        (**self).bytes_total(bytes);
    }
    fn bytes_processed(&self, bytes: u64) {
        (**self).bytes_processed(bytes);
    }
    fn is_cancelled(&self) -> bool {
        (**self).is_cancelled()
    }
    fn entry_start(&self, path: &Path, current: usize, total: usize) {
        (**self).entry_start(path, current, total);
    }
    fn entry_skipped(&self, path: &Path, reason: &str) {
        (**self).entry_skipped(path, reason);
    }
    fn entries_completed(&self, extracted: usize, skipped: usize) {
        (**self).entries_completed(extracted, skipped);
    }
    fn pfs_start(&self, total: usize) {
        (**self).pfs_start(total);
    }
    fn pfs_directory(&self, path: &Path) {
        (**self).pfs_directory(path);
    }
    fn pfs_file(&self, path: &Path, size: u64) {
        (**self).pfs_file(path, size);
    }
    fn pfs_file_completed(&self, written: u64) {
        (**self).pfs_file_completed(written);
    }
    fn pfs_completed(&self) {
        (**self).pfs_completed();
    }
}

/// A no-op progress implementation that discards all updates.
pub struct SilentProgress;

impl ExtractProgress for SilentProgress {}

/// Console progress with byte counts and throughput. All output goes to stderr.
/// Quiet mode suppresses progress, but always retains skipped-entry warnings.
#[cfg(feature = "cli")]
pub struct ConsoleProgress {
    bar: indicatif::ProgressBar,
    quiet: bool,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "cli")]
impl ConsoleProgress {
    pub fn new() -> Self {
        Self::with_options(
            false,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
    }

    pub fn with_options(
        quiet: bool,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        let bar = indicatif::ProgressBar::hidden();
        if !quiet {
            bar.set_draw_target(indicatif::ProgressDrawTarget::stderr());
            bar.set_style(indicatif::ProgressStyle::default_bar()
                .template("{msg} {bar:30.cyan/blue} {bytes}/{total_bytes} {bytes_per_sec} [{elapsed_precise}]")
                .expect("static progress template").progress_chars("━╸─"));
        }
        Self {
            bar,
            quiet,
            cancelled,
        }
    }
}

#[cfg(feature = "cli")]
impl Default for ConsoleProgress {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "cli")]
impl ExtractProgress for ConsoleProgress {
    fn phase(&self, name: &str) {
        if !self.quiet {
            if self.bar.is_finished() {
                self.bar.reset();
            }
            self.bar.set_message(name.to_owned());
            if self.bar.is_hidden() {
                let _ = std::io::Write::write_fmt(
                    &mut std::io::stderr().lock(),
                    format_args!("{name}…\n"),
                );
            }
        }
    }
    fn bytes_total(&self, bytes: u64) {
        self.bar.set_position(0);
        self.bar.reset_elapsed();
        self.bar.set_length(bytes);
    }
    fn bytes_processed(&self, bytes: u64) {
        self.bar.inc(bytes);
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn entry_skipped(&self, path: &Path, reason: &str) {
        let warning = format!(
            "warning: skipped {}: {}",
            path.display().to_string().escape_debug(),
            reason.escape_debug()
        );
        if self.bar.is_hidden() {
            let _ = std::io::Write::write_fmt(
                &mut std::io::stderr().lock(),
                format_args!("{warning}\n"),
            );
        } else {
            self.bar.println(warning);
        }
    }
    fn pfs_completed(&self) {
        self.bar.finish_and_clear();
    }
}

#[cfg(feature = "cli")]
impl Drop for ConsoleProgress {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}
