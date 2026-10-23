use crate::cli::{self, Cli, Command};
use crate::output::{diagnostic, error_message, escaped, human_size};
use clap::CommandFactory;
use orbis_cli::{
    ConsoleProgress, ExtractError, ExtractOptions, ListedItem, PkgExtractor, SilentProgress,
};
use serde_json::{Value, json};
use snafu::{ResultExt, Snafu};
use std::error::Error as StdError;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

mod directory;
mod export;
mod info;
mod install;
mod verify;

type Result<T> = std::result::Result<T, Box<dyn StdError>>;
type Package = orbis_pkg::Pkg<memmap2::Mmap>;

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("invalid title ID; specify an output directory with --output"))]
    InvalidTitleId,
    #[snafu(display("cannot open '{}'", path.display()))]
    OpenPkg {
        path: PathBuf,
        source: orbis_cli::OpenPkgError,
    },
}

#[derive(Clone, Copy)]
enum ExtractionMode {
    Create,
    Replace,
    Resume,
}

enum Operation<'a> {
    Verify,
    Extract {
        output: Option<&'a Path>,
        mode: ExtractionMode,
    },
}

pub(crate) fn run(cli: &Cli, out: &mut impl Write, cancelled: Arc<AtomicBool>) -> Result<u8> {
    match &cli.command {
        Command::Export {
            base,
            output,
            resume,
            dry_run,
            json,
        } => {
            if *dry_run {
                export::preview(base, output, *json, cli, out, cancelled)
            } else {
                export::run(base, output, *resume, cli, cancelled)
            }
        }
        Command::Install {
            pkg_path,
            base,
            dry_run,
            json,
            merge,
        } => install::run(
            pkg_path,
            base,
            install::InstallOptions {
                dry_run: *dry_run,
                json: *json,
                mode: if *merge {
                    orbis_cli::install::InstallMode::Merge
                } else {
                    orbis_cli::install::InstallMode::Separate
                },
            },
            cli,
            out,
            cancelled,
        ),
        Command::Status { base, json } => install::status(base, *json, cli, out, cancelled),
        Command::Rollback { base } => install::rollback(base, cli, cancelled),
        Command::Recover { base } => install::recover(base, cli, cancelled),
        Command::Info {
            pkg_path,
            json,
            header_only,
            sfo,
        } => info::run(pkg_path, *json, *header_only, *sfo, cli, out),
        Command::List {
            pkg_path,
            entries,
            files,
            json,
        } => list(pkg_path, *entries, *files, *json, cli, out),
        Command::Extract {
            pkg_path,
            output,
            force,
            resume,
            selection,
        } => process(
            pkg_path,
            Operation::Extract {
                output: output.as_deref(),
                mode: if *resume {
                    ExtractionMode::Resume
                } else if *force {
                    ExtractionMode::Replace
                } else {
                    ExtractionMode::Create
                },
            },
            selection,
            cli,
            cancelled,
        ),
        Command::Verify {
            pkg_path,
            selection,
            integrity,
            json,
        } => {
            if *integrity {
                verify::run(
                    pkg_path,
                    selection,
                    cli,
                    (*json).then_some(out as &mut dyn Write),
                    cancelled,
                )
            } else {
                process(pkg_path, Operation::Verify, selection, cli, cancelled)
            }
        }
        Command::VerifyDir { directory, json } => {
            directory::verify(directory, *json, cli, out, cancelled)
        }
        Command::Completions { shell } => {
            // Generators assume infallible writes; buffer first so a closed pipe
            // is handled by our fallible stdout path instead of panicking.
            let mut script = Vec::new();
            clap_complete::generate(
                *shell,
                &mut Cli::command(),
                cli::invoked_name(),
                &mut script,
            );
            out.write_all(&script)?;
            Ok(0)
        }
    }
}

fn open(path: &Path, passcode_file: Option<&Path>) -> Result<Package> {
    // As with the previous extractor, the input must remain unchanged while mapped.
    let mut pkg = unsafe { orbis_cli::inspect_pkg(path).context(OpenPkgSnafu { path })? };
    if let Some(path) = passcode_file {
        let reader: Box<dyn Read> = if path == Path::new("-") {
            Box::new(std::io::stdin())
        } else {
            Box::new(std::fs::File::open(path)?)
        };
        let mut bytes = Vec::new();
        reader.take(35).read_to_end(&mut bytes)?;
        let passcode = bytes
            .strip_suffix(b"\r\n")
            .or_else(|| bytes.strip_suffix(b"\n"))
            .unwrap_or(&bytes);
        let result = pkg.load_passcode(passcode);
        bytes.fill(0);
        result?;
    }
    Ok(pkg)
}

fn prepare_filesystem(pkg: &mut Package) -> Result<()> {
    let image = pkg.get_pfs_image().ok_or(ExtractError::NoPfsImage)?;
    if orbis_pfs::header::read_mode(image.data)?.is_encrypted() {
        // An unencrypted IMAGE_KEY can be usable even if entry slot 3 is not.
        let _ = pkg.load_entry_keys();
        pkg.load_pfs_key()?;
    }
    Ok(())
}

fn process(
    path: &Path,
    operation: Operation<'_>,
    selection: &cli::Selection,
    cli: &Cli,
    cancelled: Arc<AtomicBool>,
) -> Result<u8> {
    let mut pkg = open(path, cli.passcode_file.as_deref())?;
    let quiet = cli.quiet;
    let options = ExtractOptions {
        metadata_only: selection.metadata_only,
        paths: selection.paths.clone(),
    };
    // Key failures do not hide readable metadata. The extractor reports every
    // selected encrypted entry that cannot be read as an explicit omission.
    let _ = pkg.load_entry_keys();
    if !selection.metadata_only {
        prepare_filesystem(&mut pkg)?;
    }
    let progress = ConsoleProgress::with_options(quiet, cancelled);
    let start = std::time::Instant::now();
    let extracting = matches!(operation, Operation::Extract { .. });
    let report = match operation {
        Operation::Extract { output, mode } => {
            let destination = match output {
                Some(path) => path.to_path_buf(),
                None => {
                    let title_id = pkg.header().content_id().title_id();
                    validate_title_id(title_id)?;
                    std::env::current_dir()?.join(title_id)
                }
            };
            if !quiet {
                diagnostic(&format!(
                    "Extracting {} to {}",
                    escaped(path),
                    escaped(&destination)
                ));
            }
            PkgExtractor::new(&pkg, progress, matches!(mode, ExtractionMode::Replace))
                .with_resume(matches!(mode, ExtractionMode::Resume))
                .extract_with_options(&destination, &options)?
        }
        Operation::Verify => {
            if !quiet {
                diagnostic(&format!("Checking {}", escaped(path)));
            }
            PkgExtractor::new(&pkg, progress, false).verify(&options)?
        }
    };
    let action = if extracting { "Extracted" } else { "Checked" };
    if !report.skipped.is_empty() {
        diagnostic(&format!(
            "Incomplete: {} files {}; {} entries skipped.",
            report.files,
            if extracting { "extracted" } else { "checked" },
            report.skipped.len()
        ));
        return Ok(3);
    }
    if !quiet {
        diagnostic(&format!(
            "{action} {} files ({}) in {:.2}s.",
            report.files,
            human_size(report.bytes),
            start.elapsed().as_secs_f64()
        ));
        if !extracting {
            diagnostic("Readability and decompression passed; authenticity was not checked.");
        }
    }
    Ok(0)
}

fn list(
    path: &Path,
    entries_only: bool,
    files_only: bool,
    as_json: bool,
    cli: &Cli,
    out: &mut impl Write,
) -> Result<u8> {
    let mut pkg = open(path, cli.passcode_file.as_deref())?;
    let mut items = if files_only {
        vec![]
    } else {
        PkgExtractor::new(&pkg, SilentProgress, false).list_entries()?
    };
    let mut warnings = Vec::new();
    if !entries_only {
        let filesystem = prepare_filesystem(&mut pkg)
            .and_then(|()| Ok(PkgExtractor::new(&pkg, SilentProgress, false).list_files()?));
        match filesystem {
            Ok(files) => items.extend(files),
            Err(error) if !files_only => warnings.push(format!("filesystem unavailable: {}; showing PKG entries only (use --entries for metadata-only inspection)", error_message(error.as_ref()))),
            Err(error) => return Err(error),
        }
    }
    items.sort_by(|a, b| a.path.cmp(&b.path).then(a.entry_id.cmp(&b.entry_id)));
    if as_json {
        let entries: Vec<Value> = items.iter().map(|item| json!({
            "path": item.path, "source": item_source(item),
            "kind": if item.is_directory { "directory" } else { "file" },
            "size": item.size, "entry_id": item.entry_id,
            "entry_encrypted": item.entry_id.map(|_| item.encrypted), "key_index": item.key_index,
        })).collect();
        write_json(
            out,
            &json!({ "schema_version": 1, "package": path.to_string_lossy(), "items": entries, "warnings": warnings }),
        )?;
    } else {
        writeln!(out, "Contents of {}:", escaped(path))?;
        writeln!(out, "{:<10} {:<9} {:>11}  Path", "Source", "Type", "Size")?;
        for item in &items {
            let name = item.path.clone().unwrap_or_else(|| {
                format!(
                    "(unknown entry 0x{:08X}; no extraction path)",
                    item.entry_id.unwrap()
                )
            });
            writeln!(
                out,
                "{:<10} {:<9} {:>11}  {}{}",
                item_source(item),
                if item.is_directory {
                    "directory"
                } else {
                    "file"
                },
                if item.is_directory {
                    "-".into()
                } else {
                    human_size(item.size)
                },
                name.escape_debug(),
                if item.is_directory { "/" } else { "" }
            )?;
        }
    }
    for warning in &warnings {
        diagnostic(&format!("warning: {warning}"));
    }
    Ok(if warnings.is_empty() { 0 } else { 3 })
}

fn item_source(item: &ListedItem) -> &'static str {
    if item.entry_id.is_some() {
        "metadata"
    } else {
        "filesystem"
    }
}

fn write_json(out: &mut impl Write, value: &impl serde::Serialize) -> Result<()> {
    // Serialize before writing so serialization failures cannot leave half JSON.
    let bytes = serde_json::to_vec_pretty(value)?;
    out.write_all(&bytes)?;
    writeln!(out)?;
    Ok(())
}

fn validate_title_id(title_id: &str) -> Result<()> {
    if title_id.len() == 9 && title_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(Error::InvalidTitleId.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_serialization_failure_leaves_output_untouched() {
        struct FailingReport;
        impl serde::Serialize for FailingReport {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                use serde::ser::SerializeStruct;
                let mut report = serializer.serialize_struct("Report", 2)?;
                report.serialize_field("schema_version", &1)?;
                Err(serde::ser::Error::custom(
                    "cannot serialize remaining fields",
                ))
            }
        }
        let mut out = Vec::new();
        assert!(write_json(&mut out, &FailingReport).is_err());
        assert!(out.is_empty());
    }

    #[test]
    fn default_output_requires_a_valid_title_id() {
        assert!(validate_title_id("CUSA03173").is_ok());
        for title in [
            "",
            "..",
            "../escape",
            "/tmp/evil",
            "C:\\escape",
            "CUSA\0evil",
        ] {
            assert!(validate_title_id(title).is_err(), "accepted {title:?}");
        }
    }
}
