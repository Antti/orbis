//! Desktop operations use the same parser, extractor, and installer as the CLI.
use orbis_cli::{ExtractError, ExtractOptions, ExtractProgress, PkgExtractor, install};
use orbis_pkg::{
    entry::EntryId,
    sfo::{Sfo, Value},
};
use std::{
    error::Error,
    path::{Path, PathBuf},
};

mod passcode;
mod verification;
pub use passcode::Passcode;
pub use verification::VerificationReport;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug)]
pub struct PackageInfo {
    pub path: PathBuf,
    pub title: String,
    pub title_id: String,
    pub version: Option<String>,
    pub package_version: Option<String>,
    pub required_system: Option<String>,
    pub content_id: String,
    pub region: Option<&'static str>,
    pub metadata_entries: usize,
    pub missing_keys: usize,
    pub artwork: Option<eframe::egui::ColorImage>,
    pub kind: String,
    pub size: u64,
    pub update_supported: bool,
    pub update_note: String,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum Operation {
    Inspect,
    Verify {
        metadata_only: bool,
    },
    PreviewExport {
        base: PathBuf,
        output: PathBuf,
        resume: bool,
    },
    Export {
        base: PathBuf,
        preview: Box<install::ExportPlan>,
        resume: bool,
    },
    Extract {
        output: PathBuf,
        resume: bool,
    },
    PreviewUpdate {
        base: PathBuf,
        mode: install::InstallMode,
    },
    CheckGame {
        base: PathBuf,
    },
    RestoreBase {
        base: PathBuf,
    },
    RecoverBase {
        base: PathBuf,
    },
    InstallUpdate {
        base: PathBuf,
        preview: Box<install::InstallPlan>,
    },
}

impl Operation {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Inspect => "Reading package",
            Self::Verify { .. } => "Verifying package",
            Self::PreviewExport { .. } => "Checking export",
            Self::Export { .. } => "Exporting merged game",
            Self::Extract { .. } => "Extracting package",
            Self::PreviewUpdate { .. } => "Checking update",
            Self::InstallUpdate { .. } => "Installing update",
            Self::CheckGame { .. } => "Checking game folder",
            Self::RestoreBase { .. } => "Restoring base game",
            Self::RecoverBase { .. } => "Recovering game folder",
        }
    }

    pub fn failure_help(&self) -> &'static str {
        match self {
            Self::Verify { .. } => {
                "Review the report for failed or unavailable checks. Metadata checks do not verify game data."
            }
            Self::PreviewExport { .. } | Self::Export { .. } => {
                "Choose an original base with a managed separate update, or the base alone. Close the emulator. Resume checks existing files and preserves conflicts. Do not use an export until verification succeeds."
            }
            Self::Inspect => {
                "Choose a PlayStation 4 PKG file. Other .pkg formats are not supported."
            }
            Self::Extract { .. } => {
                "Completed files remain in the destination. To retry, select the same folder and enable Resume. Conflicting files are preserved; choose a new folder if needed."
            }
            Self::PreviewUpdate { .. } => {
                "Select the extracted base game's folder containing sce_sys/param.sfo and a matching full-file update PKG."
            }
            Self::InstallUpdate { .. }
            | Self::RestoreBase { .. }
            | Self::RecoverBase { .. }
            | Self::CheckGame { .. } => {
                "Keep the emulator closed and retain the backup folder beside your game. Use Manage game to check for a merged update or recover an interrupted merge. Review the error details before retrying."
            }
        }
    }
}

#[derive(Debug)]
pub enum Outcome {
    Inspected(PackageInfo),
    Verified(Box<VerificationReport>),
    ExportPreview(Box<install::ExportPlan>),
    Preview(install::InstallPlan),
    GameState(install::MergeStatus),
    Merged {
        state: install::MergeStatus,
        output: PathBuf,
    },
    Finished {
        title: String,
        message: String,
        output: PathBuf,
        incomplete: bool,
    },
}

#[derive(Debug)]
pub struct Failure {
    pub cancelled: bool,
    pub details: String,
}

#[derive(Debug)]
struct ExportVerificationError {
    output: PathBuf,
    source: install::InstallError,
}

impl std::fmt::Display for ExportVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Export created at {}, but verification did not finish. Resume and verify before using it.",
            self.output.display()
        )
    }
}

impl Error for ExportVerificationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

impl Failure {
    pub fn from_error(error: &(dyn Error + 'static)) -> Self {
        let mut parts = Vec::new();
        let mut current = Some(error);
        let mut cancelled = false;
        while let Some(error) = current {
            cancelled |= error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::Interrupted);
            cancelled |= matches!(
                error.downcast_ref::<ExtractError>(),
                Some(ExtractError::Cancelled)
            );
            let message = error.to_string();
            if parts.last() != Some(&message) {
                parts.push(message);
            }
            current = error.source();
        }
        Self {
            cancelled,
            details: parts.join("\n"),
        }
    }
}

#[cfg(test)]
pub fn run(
    path: &Path,
    operation: Operation,
    progress: &impl ExtractProgress,
) -> std::result::Result<Outcome, Failure> {
    run_with_passcode(path, operation, None, progress)
}

pub fn run_with_passcode(
    path: &Path,
    operation: Operation,
    passcode: Option<&Passcode>,
    progress: &impl ExtractProgress,
) -> std::result::Result<Outcome, Failure> {
    if let Operation::Verify { metadata_only } = operation {
        return Ok(Outcome::Verified(Box::new(verification::verify(
            path,
            metadata_only,
            passcode,
            progress,
        ))));
    }
    execute(path, operation, passcode, progress).map_err(|e| Failure::from_error(e.as_ref()))
}

fn execute(
    path: &Path,
    operation: Operation,
    passcode: Option<&Passcode>,
    progress: &impl ExtractProgress,
) -> Result<Outcome> {
    if progress.is_cancelled() {
        return Err(ExtractError::Cancelled.into());
    }
    // Restoring and recovering a game never require the original package.
    match &operation {
        Operation::PreviewExport {
            base,
            output,
            resume,
        } => {
            return Ok(Outcome::ExportPreview(Box::new(
                install::plan_export_with_resume(base, output, *resume, progress)?,
            )));
        }
        Operation::Export {
            base,
            preview,
            resume,
        } => {
            let fresh =
                install::plan_export_with_resume(base, &preview.destination, *resume, progress)?;
            if !same_export(preview, &fresh) {
                return Err(
                    "The game or destination changed after the preview. Check the export again."
                        .into(),
                );
            }
            let exported =
                install::export_for_verification(base, &preview.destination, *resume, progress)?;
            let checked = exported
                .verify(progress)
                .map_err(|source| ExportVerificationError {
                    output: exported.report().destination.clone(),
                    source,
                })?;
            let exported = exported.into_report();
            let incomplete = !checked.complete || !checked.issues.is_empty();
            return Ok(Outcome::Finished {
                title: if incomplete {
                    "Export verification failed"
                } else {
                    "Export complete and verified"
                }
                .into(),
                message: if incomplete {
                    format!(
                        "The exported directory has {} verification issues. Preserve it and review it with verify-dir before use.",
                        checked.issues.len()
                    )
                } else {
                    format!(
                        "{} version {}: {} files ({}), verified against the export manifest. The base and installed update were preserved.",
                        exported.title_id,
                        exported.app_version,
                        checked.checked_files,
                        human_size(exported.bytes)
                    )
                },
                output: exported.destination,
                incomplete,
            });
        }
        Operation::CheckGame { base } | Operation::PreviewUpdate { base, .. } => {
            if let Some(state) = install::merge_status(base)? {
                return Ok(Outcome::GameState(state));
            }
            if matches!(operation, Operation::CheckGame { .. }) {
                return Ok(Outcome::Finished {
                    title: "No merged update".into(),
                    message: "This folder has no managed merge to restore. Open an update PKG to install one.".into(),
                    output: base.clone(), incomplete: false,
                });
            }
        }
        Operation::RestoreBase { base } | Operation::RecoverBase { base } => {
            if matches!(operation, Operation::RestoreBase { .. }) {
                install::restore_merged(base, progress)?;
            } else {
                install::recover_with_progress(base, progress)?;
                if let Some(state) = install::merge_status(base)? {
                    return Ok(Outcome::GameState(state));
                }
            }
            return Ok(Outcome::Finished {
                title: "Game folder ready".into(),
                message: "The original base files have been restored, or no pending restoration was needed. You can now install another update.".into(),
                output: base.clone(), incomplete: false,
            });
        }
        _ => {}
    }
    // Same mmap contract as the CLI: the selected input must remain unchanged
    // during an operation. The GUI never writes to the input package.
    let mut pkg = unsafe { orbis_cli::inspect_pkg(path)? };
    if let Some(passcode) = passcode {
        pkg.load_passcode(passcode.bytes())?;
    }
    let _ = pkg.load_entry_keys();
    if matches!(operation, Operation::Inspect) {
        let header = pkg.header();
        let mut info = PackageInfo {
            path: path.to_owned(),
            title: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            title_id: header.content_id().title_id().to_owned(),
            version: None,
            package_version: None,
            required_system: None,
            content_id: header.content_id().as_str().to_owned(),
            region: content_region(header.content_id().service_id()),
            metadata_entries: header.entry_count(),
            missing_keys: 0,
            artwork: None,
            kind: header.content_type_name().to_owned(),
            size: header.pkg_size(),
            update_supported: false,
            update_note: String::new(),
            warnings: Vec::new(),
        };
        match pkg.param_sfo_data() {
            Ok(Some(data)) => match Sfo::parse(&data) {
                Ok(sfo) => {
                    apply_sfo(&mut info, &sfo);
                }
                Err(e) => info
                    .warnings
                    .push(format!("Package details are unavailable: {e}")),
            },
            Ok(None) => info
                .warnings
                .push("This package has no application metadata.".into()),
            Err(e) => info
                .warnings
                .push(format!("Package details are unavailable: {e}")),
        }
        match install::update_metadata(&pkg) {
            Ok(_) => info.update_supported = true,
            Err(e) => info.update_note = e.to_string(),
        }
        for entry in pkg.entries() {
            // A malformed table can declare billions of absent entries. Stop
            // at the first error; param_sfo_data already reports that failure.
            let Ok((_, entry)) = entry else { break };
            if entry.is_encrypted() && !pkg.has_entry_key(entry.key_index()) {
                info.missing_keys += 1;
            }
        }
        if let Ok((entry, _)) = pkg.find_entry(EntryId::Icon0Png)
            && entry.data_size() <= 4 * 1024 * 1024
            && let Ok(bytes) = pkg.entry_data(&entry)
        {
            info.artwork = decode_artwork(&bytes);
        }
        return Ok(Outcome::Inspected(info));
    }

    if matches!(
        operation,
        Operation::PreviewUpdate { .. } | Operation::InstallUpdate { .. }
    ) {
        install::update_metadata(&pkg)?;
    }
    let image = pkg.get_pfs_image().ok_or(ExtractError::NoPfsImage)?;
    if orbis_pfs::header::read_mode(image.data)?.is_encrypted() {
        pkg.load_pfs_key()?;
    }
    if progress.is_cancelled() {
        return Err(ExtractError::Cancelled.into());
    }
    match operation {
        Operation::Inspect
        | Operation::Verify { .. }
        | Operation::PreviewExport { .. }
        | Operation::Export { .. }
        | Operation::CheckGame { .. }
        | Operation::RestoreBase { .. }
        | Operation::RecoverBase { .. } => unreachable!(),
        Operation::Extract { output, resume } => {
            // Never overwrite a different file, including when resuming.
            let report = PkgExtractor::new(&pkg, progress, false)
                .with_resume(resume)
                .extract_with_options(&output, &ExtractOptions::default())?;
            let incomplete = !report.skipped.is_empty();
            Ok(Outcome::Finished {
                title: if incomplete {
                    "Extraction incomplete"
                } else {
                    "Extraction complete"
                }
                .into(),
                message: if incomplete {
                    format!(
                        "Extracted {} files. Skipped entries: {}. Review the warnings; this output is incomplete.",
                        report.files,
                        report.skipped.len()
                    )
                } else {
                    format!(
                        "Extracted {} files ({}).",
                        report.files,
                        human_size(report.bytes)
                    )
                },
                output,
                incomplete,
            })
        }
        Operation::PreviewUpdate { base, mode } => Ok(Outcome::Preview(install::plan_with_mode(
            &pkg, &base, mode, progress,
        )?)),
        Operation::InstallUpdate { base, preview } => {
            let fresh = install::plan_with_mode(&pkg, &base, preview.mode, progress)?;
            if !same_update(&preview, &fresh) {
                return Err("The package or installed game changed after the preview. Check the update again before installing.".into());
            }
            let plan = install::install_with_mode(&pkg, &base, preview.mode, progress)?;
            if plan.mode == install::InstallMode::Merge {
                return Ok(Outcome::Merged {
                    state: install::MergeStatus {
                        title_id: plan.title_id,
                        base_version: plan.current_version,
                        update_version: plan.update_version,
                        pending_recovery: false,
                    },
                    output: plan.destination,
                });
            }
            Ok(Outcome::Finished {
                title: "Update installed".into(),
                message: format!(
                    "{} is now at version {}. The base game was preserved. One previous installation is available through the CLI's rollback command.",
                    plan.title_id, plan.update_version
                ),
                output: plan.destination,
                incomplete: false,
            })
        }
    }
}

fn apply_sfo(info: &mut PackageInfo, sfo: &Sfo<'_>) {
    if let Ok(title) = sfo.string("TITLE") {
        info.title = title.to_owned();
    }
    info.version = sfo.string("APP_VER").ok().map(str::to_owned);
    info.package_version = sfo.string("VERSION").ok().map(str::to_owned);
    info.required_system = match sfo.get("SYSTEM_VER") {
        Some(Value::Integer(value)) => {
            Some(format!("{:x}.{:02x}", value >> 24, (value >> 16) & 0xff))
        }
        _ => None,
    };
    match sfo.string("CATEGORY").unwrap_or("") {
        "gd" => info.kind = "Base game".into(),
        "gde" => info.kind = "Application".into(),
        "gp" => info.kind = "Game update".into(),
        "ac" => info.kind = "Add-on".into(),
        _ => {}
    }
}

fn content_region(service: &str) -> Option<&'static str> {
    match service {
        "UP" => Some("North America"),
        "EP" => Some("Europe"),
        "JP" => Some("Japan"),
        "HP" => Some("Asia"),
        _ => None,
    }
}

fn decode_artwork(bytes: &[u8]) -> Option<eframe::egui::ColorImage> {
    // Artwork is optional, untrusted package data. Bound both input and decoded
    // dimensions before allocating, and never let a bad icon block extraction.
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?.thumbnail(160, 160).into_rgba8();
    Some(eframe::egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

fn same_export(a: &install::ExportPlan, b: &install::ExportPlan) -> bool {
    a.title_id == b.title_id
        && a.base_version == b.base_version
        && a.update_version == b.update_version
        && a.destination == b.destination
        && a.files == b.files
        && a.update_files == b.update_files
        && a.bytes == b.bytes
}

fn same_update(a: &install::InstallPlan, b: &install::InstallPlan) -> bool {
    a.mode == b.mode
        && a.backup_bytes == b.backup_bytes
        && a.title_id == b.title_id
        && a.content_id == b.content_id
        && a.current_version == b.current_version
        && a.update_version == b.update_version
        && a.destination == b.destination
        && a.extracted_bytes == b.extracted_bytes
        && a.files.len() == b.files.len()
        && a.files
            .iter()
            .zip(&b.files)
            .all(|(a, b)| a.path == b.path && a.change == b.change)
}

pub fn folder_name_valid(name: &str) -> bool {
    // A deliberately small, portable set: package metadata cannot introduce paths.
    !name.is_empty()
        && name.len() <= 100
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && !matches!(
            name.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
}

pub fn suggested_folder(info: &PackageInfo) -> String {
    if folder_name_valid(&info.title_id) {
        info.title_id.clone()
    } else {
        "extracted-game".into()
    }
}

pub fn human_size(bytes: u64) -> String {
    let mut size = bytes as f64;
    let mut unit = "B";
    for next in ["KiB", "MiB", "GiB", "TiB"] {
        if size < 1024.0 {
            break;
        }
        size /= 1024.0;
        unit = next;
    }
    format!("{size:.1} {unit}")
}

#[cfg(test)]
mod tests;
