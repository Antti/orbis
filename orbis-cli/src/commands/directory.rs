//! Portable exported-directory verification output.
use super::*;

pub(super) fn verify(
    directory: &Path,
    json: bool,
    cli: &Cli,
    out: &mut impl Write,
    cancelled: Arc<AtomicBool>,
) -> Result<u8> {
    let report = orbis_cli::install::verify_directory(
        directory,
        ConsoleProgress::with_options(cli.quiet, cancelled),
    )?;
    if json {
        write_json(out, &report)?;
    } else {
        writeln!(
            out,
            "Checked {} files; {} issues; export {}",
            report.checked_files,
            report.issues.len(),
            if report.complete {
                "completed"
            } else {
                "incomplete"
            }
        )?;
        for issue in &report.issues {
            writeln!(out, "{}: {}", issue.kind, issue.path.escape_debug())?;
        }
    }
    Ok(if !report.issues.is_empty() {
        1
    } else if !report.complete {
        3
    } else {
        0
    })
}
