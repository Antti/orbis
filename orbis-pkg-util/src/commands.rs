use crate::cli::{self, Cli, Command};
use crate::output::{diagnostic, error_message, escaped, human_size};
use clap::CommandFactory;
use orbis_pkg_util::{
    ConsoleProgress, ExtractError, ExtractOptions, ListedItem, PkgExtractor, SilentProgress,
};
use serde_json::{Value, json};
use snafu::{ResultExt, Snafu};
use std::error::Error as StdError;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

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
        source: orbis_pkg_util::OpenPkgError,
    },
}

pub(crate) fn run(cli: &Cli, out: &mut impl Write, cancelled: Arc<AtomicBool>) -> Result<u8> {
    match &cli.command {
        Command::Install {
            pkg_path,
            base,
            dry_run,
            json,
        } => {
            let mut pkg = open(pkg_path, cli.passcode_file.as_deref())?;
            let _ = pkg.load_entry_keys();
            orbis_pkg_util::install::update_metadata(&pkg)?;
            prepare_filesystem(&mut pkg)?;
            if *dry_run {
                let plan = orbis_pkg_util::install::plan_with_progress(
                    &pkg,
                    base,
                    ConsoleProgress::with_options(cli.quiet, cancelled),
                )?;
                if *json {
                    write_json(out, &json!({ "schema_version": 1, "plan": plan }))?;
                } else {
                    writeln!(
                        out,
                        "{}: {} -> {}",
                        plan.title_id, plan.current_version, plan.update_version
                    )?;
                    writeln!(out, "Destination: {}", escaped(&plan.destination))?;
                    writeln!(
                        out,
                        "Temporary space: approximately {} ({} available)",
                        human_size(plan.required_free_bytes),
                        human_size(plan.available_bytes)
                    )?;
                    for file in plan.files {
                        let action = match file.change {
                            orbis_pkg_util::install::ChangeKind::Add => "add",
                            orbis_pkg_util::install::ChangeKind::Replace => "replace",
                            orbis_pkg_util::install::ChangeKind::RemoveFromUpdate => {
                                "remove from update"
                            }
                        };
                        writeln!(out, "{action}: {}", file.path.escape_debug())?;
                    }
                    writeln!(
                        out,
                        "The base stays unchanged; paths removed from the update fall back to the base."
                    )?;
                }
            } else {
                let progress = ConsoleProgress::with_options(cli.quiet, cancelled);
                let plan = orbis_pkg_util::install::install(&pkg, base, progress)?;
                if !cli.quiet {
                    diagnostic(&format!(
                        "Installed {} version {} to {}. Previous installation is available with rollback --base {}.",
                        plan.title_id,
                        plan.update_version,
                        escaped(&plan.destination),
                        escaped(base)
                    ));
                }
            }
            Ok(0)
        }
        Command::Rollback { base } => {
            orbis_pkg_util::install::rollback_with_progress(
                base,
                ConsoleProgress::with_options(cli.quiet, cancelled),
            )?;
            if !cli.quiet {
                diagnostic("Previous installation restored.");
            }
            Ok(0)
        }
        Command::Recover { base } => {
            orbis_pkg_util::install::recover_with_progress(
                base,
                ConsoleProgress::with_options(cli.quiet, cancelled),
            )?;
            if !cli.quiet {
                diagnostic("Recovery and staging cleanup completed.");
            }
            Ok(0)
        }
        Command::Info {
            pkg_path,
            json,
            header_only,
            sfo,
        } => info(pkg_path, *json, *header_only, *sfo, cli, out),
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
            selection,
        } => process(
            pkg_path,
            Some((output.as_deref(), *force)),
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
                process(pkg_path, None, selection, cli, cancelled)
            }
        }
        Command::Completions { shell } => {
            // Generators assume infallible writes; buffer first so a closed pipe
            // is handled by our fallible stdout path instead of panicking.
            let mut script = Vec::new();
            clap_complete::generate(*shell, &mut Cli::command(), "orbis-pkg-util", &mut script);
            out.write_all(&script)?;
            Ok(0)
        }
    }
}

fn open(path: &Path, passcode_file: Option<&Path>) -> Result<Package> {
    // As with the previous extractor, the input must remain unchanged while mapped.
    let mut pkg = unsafe { orbis_pkg_util::inspect_pkg(path).context(OpenPkgSnafu { path })? };
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
    extraction: Option<(Option<&Path>, bool)>,
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
    let report = if let Some((output, force)) = extraction {
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
        PkgExtractor::new(&pkg, progress, force).extract_with_options(&destination, &options)?
    } else {
        if !quiet {
            diagnostic(&format!("Checking {}", escaped(path)));
        }
        PkgExtractor::new(&pkg, progress, false).verify(&options)?
    };
    let action = if extraction.is_some() {
        "Extracted"
    } else {
        "Checked"
    };
    if !report.skipped.is_empty() {
        diagnostic(&format!(
            "Incomplete: {} files {}; {} entries skipped.",
            report.files,
            if extraction.is_some() {
                "extracted"
            } else {
                "checked"
            },
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
        if extraction.is_none() {
            diagnostic("Readability and decompression passed; authenticity was not checked.");
        }
    }
    Ok(0)
}

fn info(
    path: &Path,
    as_json: bool,
    header_only: bool,
    all_sfo: bool,
    cli: &Cli,
    out: &mut impl Write,
) -> Result<u8> {
    let mut pkg = open(
        path,
        if header_only {
            None
        } else {
            cli.passcode_file.as_deref()
        },
    )?;
    let (application, sfo, warnings) = if header_only {
        (Value::Null, Value::Null, Vec::new())
    } else {
        let _ = pkg.load_entry_keys();
        application_info(&pkg)
    };
    let header = pkg.header();
    let id = header.content_id();
    if as_json {
        write_json(
            out,
            &json!({
                "schema_version": 1,
                "package": path.to_string_lossy(),
                "application": application, "sfo": sfo, "warnings": warnings,
                "metadata": {
                    "content_id": id.as_str(), "service_id": id.service_id(),
                    "publisher_code": id.publisher_code(), "title_id": id.title_id(),
                    "content_version": id.version(), "label": id.label(),
                    "content_type": header.content_type(), "content_type_name": header.content_type_name(),
                    "content_flags": header.content_flags().bits(), "content_flags_name": header.content_flags().to_string(),
                    "drm_type": header.drm_type(), "drm_type_name": header.drm_type_name(),
                    "pkg_type": header.pkg_type(), "pkg_size": header.pkg_size(),
                    "file_count": header.file_count(), "entry_count": header.entry_count(),
                    "table_offset": header.table_offset(), "pfs_offset": header.pfs_offset(), "pfs_size": header.pfs_size(),
                }
            }),
        )?;
    } else {
        writeln!(out, "PKG: {}\n", escaped(path))?;
        writeln!(out, "Content ID:       {}", id.as_str().escape_debug())?;
        writeln!(out, "  Service ID:     {}", id.service_id().escape_debug())?;
        writeln!(
            out,
            "  Publisher:      {}",
            id.publisher_code().escape_debug()
        )?;
        writeln!(out, "  Title ID:       {}", id.title_id().escape_debug())?;
        writeln!(out, "  Content version: {}", id.version().escape_debug())?;
        writeln!(out, "  Label:          {}", id.label().escape_debug())?;
        writeln!(
            out,
            "Content Type:     0x{:02X} ({})",
            header.content_type(),
            header.content_type_name()
        )?;
        writeln!(out, "Content Flags:    {}", header.content_flags())?;
        writeln!(
            out,
            "DRM Type:         0x{:02X} ({})",
            header.drm_type(),
            header.drm_type_name()
        )?;
        writeln!(out, "PKG Type:         0x{:08X}", header.pkg_type())?;
        writeln!(
            out,
            "PKG Size:         {} ({} bytes)",
            human_size(header.pkg_size()),
            header.pkg_size()
        )?;
        writeln!(out, "File Count:       {}", header.file_count())?;
        writeln!(out, "Entry Count:      {}", header.entry_count())?;
        writeln!(out, "Table Offset:     0x{:X}", header.table_offset())?;
        writeln!(out, "PFS Offset:       0x{:X}", header.pfs_offset())?;
        writeln!(
            out,
            "PFS Size:         {} ({} bytes)",
            human_size(header.pfs_size() as u64),
            header.pfs_size()
        )?;
    }
    if !as_json && !application.is_null() {
        writeln!(out, "\nApplication:")?;
        for (key, value) in application.as_object().unwrap() {
            if !value.is_null() {
                writeln!(
                    out,
                    "  {key}: {}",
                    value
                        .as_str()
                        .map(|s| s.escape_debug().to_string())
                        .unwrap_or_else(|| value.to_string())
                )?;
            }
        }
        if all_sfo {
            writeln!(out, "\nPARAM.SFO:")?;
            for (key, value) in sfo.as_object().unwrap() {
                writeln!(out, "  {}: {}", key.escape_debug(), value)?;
            }
        }
    }
    for warning in &warnings {
        diagnostic(&format!("warning: {warning}"));
    }
    Ok(if warnings.is_empty() { 0 } else { 3 })
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

fn application_info(pkg: &Package) -> (Value, Value, Vec<String>) {
    use orbis_pkg::{
        entry::EntryId,
        sfo::{Sfo, Value as SfoValue},
    };
    let read = || -> Result<(Value, Value)> {
        let mut found = None;
        for result in pkg.entries() {
            let (_, entry) = result?;
            if entry.entry_id() == EntryId::ParamSfo {
                if found.is_some() || entry.data_size() > 1024 * 1024 {
                    return Err("duplicate or oversized PARAM.SFO entry".into());
                }
                found = Some(entry);
            }
        }
        let Some(entry) = found else {
            return Ok((Value::Null, Value::Null));
        };
        let data = pkg.entry_data(&entry)?;
        let sfo = Sfo::parse(&data)?;
        let fields: serde_json::Map<String, Value> = sfo.entries().map(|(key, value)| {
            let value = match value {
                SfoValue::String(s) => json!({"type": "string", "value": s}),
                SfoValue::Integer(n) => json!({"type": "integer", "value": n}),
                SfoValue::Binary(b) => json!({"type": "binary", "hex": b.iter().map(|v| format!("{v:02x}")).collect::<String>()}),
            };
            (key.to_owned(), value)
        }).collect();
        let firmware = match sfo.get("SYSTEM_VER") {
            Some(SfoValue::Integer(n)) => Some(format!("{:x}.{:02x}", n >> 24, (n >> 16) & 0xff)),
            _ => None,
        };
        let build: serde_json::Map<String, Value> = sfo
            .string("PUBTOOLINFO")
            .unwrap_or("")
            .split(',')
            .filter_map(|field| field.split_once('='))
            .map(|(key, value)| (key.trim().to_owned(), json!(value.trim())))
            .collect();
        let region = match pkg.header().content_id().service_id() {
            "UP" => Some("North America"),
            "EP" => Some("Europe"),
            "JP" => Some("Japan"),
            "HP" => Some("Asia"),
            _ => None,
        };
        Ok((
            json!({
                "title": sfo.string("TITLE").ok(),
                "title_id": sfo.string("TITLE_ID").ok(),
                "content_id": sfo.string("CONTENT_ID").ok(),
                "app_version": sfo.string("APP_VER").ok(),
                "package_version": sfo.string("VERSION").ok(),
                "category": sfo.string("CATEGORY").ok(),
                "required_system_version": firmware,
                "region_from_content_id": region,
                "build": build,
            }),
            Value::Object(fields),
        ))
    };
    match read() {
        Ok((application, sfo)) => (application, sfo, vec![]),
        Err(error) => (
            Value::Null,
            Value::Null,
            vec![format!(
                "PARAM.SFO unavailable: {}; showing header metadata",
                error_message(error.as_ref())
            )],
        ),
    }
}

fn write_json(out: &mut impl Write, value: &Value) -> Result<()> {
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
