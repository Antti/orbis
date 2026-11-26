//! Desktop operations use the same parser, extractor, and installer as the CLI.
use orbis_cli::{ExtractError, ExtractOptions, ExtractProgress, PkgExtractor, install};
use orbis_pkg::{
    EntryDataError, ParamSfoError,
    entry::EntryId,
    header::ContentId,
    passcode::ENTRY_KEY_COUNT,
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
    pub content_id: ContentId,
    pub region: Option<&'static str>,
    pub metadata_entries: usize,
    pub artwork: Option<eframe::egui::ColorImage>,
    pub can_unlock_details: bool,
    pub kind: String,
    pub size: u64,
    pub update: install::UpdateCheck,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum UpdatePackage {
    Readable(ContentId),
    Locked(install::LockedUpdate),
}

impl UpdatePackage {
    fn game_target(&self, base: &Path) -> install::GameTarget {
        let id = match self {
            Self::Readable(id) => *id,
            Self::Locked(update) => update.content_id(),
        };
        install::GameTarget::for_update(base.to_owned(), id)
    }
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
    Export(Box<install::ExportPreview>),
    Extract {
        output: PathBuf,
        resume: bool,
    },
    PreviewUpdate {
        package: UpdatePackage,
        base: PathBuf,
        mode: install::InstallMode,
    },
    CheckGame {
        target: install::GameTarget,
    },
    RestoreBase {
        target: install::GameTarget,
    },
    RollbackSeparate {
        target: install::GameTarget,
        plan: install::SeparateRollback,
    },
    RecoverBase {
        target: install::GameTarget,
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
            Self::Export(_) => "Exporting merged game",
            Self::Extract { .. } => "Extracting package",
            Self::PreviewUpdate { .. } => "Checking update",
            Self::InstallUpdate { .. } => "Installing update",
            Self::CheckGame { .. } => "Checking game folder",
            Self::RestoreBase { .. } => "Restoring base game",
            Self::RollbackSeparate { .. } => "Rolling back separate update",
            Self::RecoverBase { .. } => "Recovering game folder",
        }
    }

    pub fn failure_help(&self) -> &'static str {
        match self {
            Self::Verify { .. } => {
                "Review the report for failed or unavailable checks. Metadata checks do not verify game data."
            }
            Self::PreviewExport { .. } | Self::Export(_) => {
                "Choose an original base with a managed separate update, or the base alone. Close the emulator. Resume checks existing files and preserves conflicts. Do not use an export until verification succeeds."
            }
            Self::Inspect => {
                "Choose a PlayStation 4 PKG file. Other .pkg formats are not supported."
            }
            Self::Extract { .. } => {
                "Completed files remain in the destination. To retry, select the same folder and enable Resume. Conflicting files are preserved; choose a new folder if needed."
            }
            Self::PreviewUpdate { .. } => {
                "Use a newer full-file update PKG with the same title and content IDs as the base game."
            }
            Self::CheckGame { .. } => {
                "Choose the extracted base game folder containing sce_sys/param.sfo. For an interrupted merge, choose the original game folder."
            }
            Self::RollbackSeparate { .. } => {
                "Keep the emulator closed. Refresh the game folder to check the installed version and available rollback target."
            }
            Self::InstallUpdate { .. } | Self::RestoreBase { .. } | Self::RecoverBase { .. } => {
                "Keep the emulator closed and retain the backup folder beside your game. Open the game folder to check its status or recover an interrupted merge."
            }
        }
    }
}

#[derive(Debug)]
pub enum OutputFolder {
    Directory(PathBuf),
    ExtractedGame(PathBuf),
}

impl OutputFolder {
    pub fn path(&self) -> &Path {
        match self {
            Self::Directory(path) | Self::ExtractedGame(path) => path,
        }
    }
}

#[derive(Debug)]
pub enum Outcome {
    Inspected(Box<PackageInfo>),
    UpdateTarget(std::result::Result<(), install::InstallError>),
    Verified(Box<VerificationReport>),
    ExportPreview(Box<install::ExportPreview>),
    Preview(install::InstallPlan),
    GameStatus {
        target: install::GameTarget,
        status: Box<install::InstallationStatus>,
        rollback: Option<install::SeparateRollback>,
    },
    GameChanged {
        target: install::GameTarget,
        title: &'static str,
        message: String,
    },
    Finished {
        title: String,
        message: String,
        output: OutputFolder,
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
                install::ExportPreview::new(base, output, *resume, progress)?,
            )));
        }
        Operation::Export(preview) => {
            let exported = preview.export(progress)?;
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
                output: OutputFolder::Directory(exported.destination),
                incomplete,
            });
        }
        Operation::PreviewUpdate {
            package,
            base,
            mode,
        } => {
            let target = package.game_target(base);
            match target.merged_status(progress) {
                Ok(Some(status)) => {
                    return Ok(Outcome::GameStatus {
                        target,
                        status: Box::new(status),
                        rollback: None,
                    });
                }
                Err(error @ install::InstallError::UpdateIdentityMismatch) => {
                    return Ok(Outcome::UpdateTarget(Err(error)));
                }
                Err(error) => return Err(error.into()),
                Ok(None) => {}
            }
            if let UpdatePackage::Locked(update) = package {
                return Ok(Outcome::UpdateTarget(update.check_target(base, *mode)));
            }
        }
        Operation::CheckGame { target } => {
            let (status, rollback) = target.status_with_rollback(progress)?;
            return Ok(Outcome::GameStatus {
                target: target.clone(),
                status: Box::new(status),
                rollback,
            });
        }
        Operation::RollbackSeparate { target, plan } => {
            plan.apply(&target.path, progress)?;
            return Ok(Outcome::GameChanged {
                target: target.clone(),
                title: "Rollback complete",
                message: format!("Version {} is now active.", plan.target_version()),
            });
        }
        Operation::RestoreBase { target } | Operation::RecoverBase { target } => {
            let (title, message) = if matches!(operation, Operation::RestoreBase { .. }) {
                target.restore_merged(progress)?;
                (
                    "Base restored",
                    "The original game files have been restored.",
                )
            } else {
                target.recover(progress)?;
                (
                    "Recovery complete",
                    "Recovery finished for this game folder.",
                )
            };
            return Ok(Outcome::GameChanged {
                target: target.clone(),
                title,
                message: message.into(),
            });
        }
        _ => {}
    }
    // Same mmap contract as the CLI: the selected input must remain unchanged
    // during an operation. The GUI never writes to the input package.
    let mut pkg = unsafe { orbis_cli::inspect_pkg(path)? };
    if let Some(passcode) = passcode {
        pkg.load_passcode_with_diagnostics(passcode.bytes())?;
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
            content_id: *header.content_id(),
            region: content_region(header.content_id().service_id()),
            metadata_entries: header.entry_count(),
            artwork: None,
            can_unlock_details: false,
            kind: header.content_type_name().to_owned(),
            size: header.pkg_size(),
            update: install::inspect_update(&pkg),
            warnings: Vec::new(),
        };
        let mut locked_details = false;
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
            Err(e) => {
                if let ParamSfoError::ReadData { source } = &e {
                    locked_details = passcode_supplies_key(source);
                }
                info.warnings
                    .push(format!("Package details are unavailable: {e}"));
            }
        }
        if let Ok((entry, _)) = pkg.find_entry(EntryId::Icon0Png)
            && entry.data_size() <= 4 * 1024 * 1024
        {
            match pkg.entry_data(&entry) {
                Ok(bytes) => info.artwork = decode_artwork(&bytes),
                Err(error) => locked_details |= passcode_supplies_key(&error),
            }
        }
        info.can_unlock_details = locked_details && pkg.check_passcode_metadata().is_ok();
        return Ok(Outcome::Inspected(Box::new(info)));
    }

    let target = match &operation {
        Operation::PreviewUpdate { base, mode, .. } => Some((base, *mode)),
        Operation::InstallUpdate { base, preview } => Some((base, preview.mode)),
        _ => None,
    };
    if let Some((base, mode)) = target {
        install::inspect_update(&pkg)
            .readiness
            .require_readable_for(base, mode)?;
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
        | Operation::Export(_)
        | Operation::CheckGame { .. }
        | Operation::RestoreBase { .. }
        | Operation::RollbackSeparate { .. }
        | Operation::RecoverBase { .. } => unreachable!(),
        Operation::Extract { output, resume } => {
            // Never overwrite a different file, including when resuming.
            let report = PkgExtractor::new(&pkg, progress, false)
                .with_resume(resume)
                .extract_with_options(&output, &ExtractOptions::default())?;
            let incomplete = !report.skipped.is_empty();
            let is_base = !incomplete
                && install::base_metadata(&output)
                    .is_ok_and(|base| base.content_id == pkg.header().content_id().as_str());
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
                output: if is_base {
                    OutputFolder::ExtractedGame(output)
                } else {
                    OutputFolder::Directory(output)
                },
                incomplete,
            })
        }
        Operation::PreviewUpdate { base, mode, .. } => Ok(Outcome::Preview(
            install::plan_with_mode(&pkg, &base, mode, progress)?,
        )),
        Operation::InstallUpdate { base, preview } => {
            let plan = preview.apply(&pkg, &base, progress)?;
            if plan.mode == install::InstallMode::Merge {
                return Ok(Outcome::GameChanged {
                    target: install::GameTarget::for_update(
                        plan.destination,
                        *pkg.header().content_id(),
                    ),
                    title: "Update merged",
                    message: format!(
                        "{} is now at version {} in the game folder. Keep the adjacent backup folder.",
                        plan.title_id, plan.update_version
                    ),
                });
            }
            Ok(Outcome::Finished {
                title: "Update installed".into(),
                message: format!(
                    "{} is now at version {}. You can restore the previous version from the game folder.",
                    plan.title_id, plan.update_version
                ),
                output: OutputFolder::Directory(plan.destination),
                incomplete: false,
            })
        }
    }
}

fn passcode_supplies_key(error: &EntryDataError) -> bool {
    matches!(error, EntryDataError::NoDecryptionKey { key_index } if *key_index < ENTRY_KEY_COUNT)
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
