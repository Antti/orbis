//! Installation operations and read-only diagnostics.
use super::*;

pub(super) fn run(
    pkg_path: &Path,
    base: &Path,
    dry_run: bool,
    json: bool,
    cli: &Cli,
    out: &mut impl Write,
    cancelled: Arc<AtomicBool>,
) -> Result<u8> {
    let mut pkg = open(pkg_path, cli.passcode_file.as_deref())?;
    let _ = pkg.load_entry_keys();
    orbis_pkg_util::install::update_metadata(&pkg)?;
    prepare_filesystem(&mut pkg)?;
    if dry_run {
        let plan = orbis_pkg_util::install::plan_with_progress(
            &pkg,
            base,
            ConsoleProgress::with_options(cli.quiet, cancelled),
        )?;
        if json {
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
                    orbis_pkg_util::install::ChangeKind::RemoveFromUpdate => "remove from update",
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

pub(super) fn status(
    base: &Path,
    json: bool,
    cli: &Cli,
    out: &mut impl Write,
    cancelled: Arc<AtomicBool>,
) -> Result<u8> {
    let report =
        orbis_pkg_util::install::status(base, ConsoleProgress::with_options(cli.quiet, cancelled))?;
    if json {
        write_json(out, &report)?;
    } else {
        writeln!(
            out,
            "{}: base {}, active update {}",
            report.title_id,
            report.base_version,
            report.active_version.as_deref().unwrap_or("none")
        )?;
        writeln!(
            out,
            "Rollback: {} ({})",
            report.rollback,
            report.rollback_version.as_deref().unwrap_or("none")
        )?;
        writeln!(
            out,
            "Base file integrity is not tracked; managed update and rollback files are checked."
        )?;
        for path in &report.unmanaged_directories {
            writeln!(out, "Unmanaged: {}", escaped(path))?;
        }
        for path in &report.modified_files {
            writeln!(out, "Modified: {}", path.escape_debug())?;
        }
        for issue in &report.issues {
            writeln!(out, "{}", issue.escape_debug())?;
        }
    }
    Ok(
        if report.issues.is_empty() && report.unmanaged_directories.is_empty() {
            0
        } else {
            3
        },
    )
}

pub(super) fn rollback(base: &Path, cli: &Cli, cancelled: Arc<AtomicBool>) -> Result<u8> {
    orbis_pkg_util::install::rollback_with_progress(
        base,
        ConsoleProgress::with_options(cli.quiet, cancelled),
    )?;
    if !cli.quiet {
        diagnostic("Previous installation restored.");
    }
    Ok(0)
}

pub(super) fn recover(base: &Path, cli: &Cli, cancelled: Arc<AtomicBool>) -> Result<u8> {
    orbis_pkg_util::install::recover_with_progress(
        base,
        ConsoleProgress::with_options(cli.quiet, cancelled),
    )?;
    if !cli.quiet {
        diagnostic("Recovery and staging cleanup completed.");
    }
    Ok(0)
}
