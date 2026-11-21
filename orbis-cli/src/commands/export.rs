//! Export planning and execution output.
use super::*;

pub(super) fn preview(
    base: &Path,
    output: &Path,
    resume: bool,
    json: bool,
    cli: &Cli,
    out: &mut impl Write,
    cancelled: Arc<AtomicBool>,
) -> Result<u8> {
    let estimate = orbis_cli::install::ExportEstimate::new(
        base,
        output,
        resume,
        ConsoleProgress::with_options(cli.quiet, cancelled),
    )?;
    let plan = &estimate.plan;
    if json {
        write_json(out, &estimate)?;
    } else {
        writeln!(
            out,
            "{}: base {}, update {}",
            plan.title_id,
            plan.base_version,
            plan.update_version.as_deref().unwrap_or("none")
        )?;
        writeln!(
            out,
            "{} files ({} from update), {}",
            plan.files,
            plan.update_files,
            human_size(plan.bytes)
        )?;
        writeln!(
            out,
            "Destination: {} ({})",
            escaped(&plan.destination),
            plan.filesystem.filesystem
        )?;
        if let Some(work) = &estimate.resume {
            writeln!(
                out,
                "Reusable files: {}. Remaining files: {} ({} to copy).",
                work.reusable_files,
                work.remaining_files,
                human_size(work.remaining_bytes)
            )?;
        }
        writeln!(
            out,
            "Peak space reserve: {} ({} available)",
            human_size(plan.filesystem.required_free_bytes),
            human_size(plan.filesystem.available_bytes)
        )?;
        if plan.filesystem.shortfall_bytes() > 0 {
            writeln!(
                out,
                "Additional free space needed: {}",
                human_size(plan.filesystem.shortfall_bytes())
            )?;
        }
        for warning in &plan.filesystem.warnings {
            writeln!(out, "{warning}")?;
        }
    }
    Ok(if plan.filesystem.shortfall_bytes() == 0 {
        0
    } else {
        3
    })
}

pub(super) fn run(
    base: &Path,
    output: &Path,
    resume: bool,
    cli: &Cli,
    cancelled: Arc<AtomicBool>,
) -> Result<u8> {
    let progress = ConsoleProgress::with_options(cli.quiet, cancelled);
    let report = if resume {
        orbis_cli::install::resume_export(base, output, progress)?
    } else {
        orbis_cli::install::export_merged(base, output, progress)?
    };
    if !cli.quiet {
        diagnostic(&format!(
            "Exported {} version {}: {} files ({}, {} from the update) to {}.",
            report.title_id,
            report.app_version,
            report.files,
            human_size(report.bytes),
            report.update_files,
            escaped(&report.destination),
        ));
    }
    Ok(0)
}
