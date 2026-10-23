use super::*;
use crate::output::has_io_kind;
use orbis_integrity::{Check, Status};
use std::{collections::BTreeMap, io, sync::atomic::Ordering};

#[derive(Default, serde::Serialize)]
struct Summary {
    digests: usize,
    macs: usize,
    unavailable: usize,
    failed: usize,
}

/// Write a single JSON document incrementally: memory is independent of the
/// number of block checks. Even input errors and cancellation close the document
/// with a status/error; a failed output stream is the unavoidable exception.
pub(super) fn run(
    path: &Path,
    selection: &cli::Selection,
    cli: &Cli,
    mut out: Option<&mut dyn Write>,
    cancelled: Arc<AtomicBool>,
) -> Result<u8> {
    if let Some(out) = out.as_deref_mut() {
        write!(
            out,
            "{{\"schema_version\":1,\"package\":{},\"metadata_only\":{},\"checks\":[",
            serde_json::to_string(&path.to_string_lossy())?,
            selection.metadata_only
        )?;
    }
    let mut summary = Summary::default();
    let mut reasons = BTreeMap::<(String, &'static str), usize>::new();
    let mut first = true;
    let result = (|| -> Result<_> {
        let mut pkg = open(path, cli.passcode_file.as_deref())?;
        let _ = pkg.load_entry_keys();
        if !selection.metadata_only {
            // Try loading the signing key even for an unencrypted signed image.
            // Missing keys are reported by the verifier when parsing is possible.
            let _ = pkg.load_pfs_key();
        }
        let progress = ConsoleProgress::with_options(cli.quiet || out.is_some(), cancelled.clone());
        let extractor = PkgExtractor::new(&pkg, progress, false);
        extractor.verify_integrity_with(selection.metadata_only, |layer, check: Check<'_>| {
            match check.status {
                Status::Passed if layer == "pkg" => summary.digests += check.count,
                Status::Passed => summary.macs += check.count,
                Status::Unavailable => {
                    summary.unavailable += check.count;
                    *reasons
                        .entry((layer.into(), check.reason.unwrap().as_str()))
                        .or_default() += check.count;
                }
                Status::Failed => summary.failed += check.count,
                Status::NotApplicable => {}
            }
            if let Some(out) = out.as_deref_mut() {
                if !first {
                    out.write_all(b",")?;
                }
                first = false;
                serde_json::to_writer(
                    out,
                    &json!({
                        "layer": layer, "scope": check.scope,
                        "algorithm": if layer == "pkg" { "sha256" } else { "hmac_sha256" },
                        "status": check.status.as_str(),
                        "reason": check.reason.map(|r| r.as_str()), "count": check.count,
                    }),
                )?;
            }
            Ok(())
        })?;
        Ok(extractor.verify(&ExtractOptions {
            metadata_only: selection.metadata_only,
            paths: Vec::new(),
        })?)
    })();
    let (status, code) = match &result {
        Ok(report) if summary.unavailable > 0 || !report.skipped.is_empty() => ("incomplete", 3),
        Ok(_) => ("complete", 0),
        Err(error)
            if cancelled.load(Ordering::Relaxed)
                || has_io_kind(error.as_ref(), io::ErrorKind::Interrupted)
                || matches!(
                    error.downcast_ref::<ExtractError>(),
                    Some(ExtractError::Cancelled)
                ) =>
        {
            ("interrupted", 130)
        }
        Err(_) => ("failed", 1),
    };
    if let Some(out) = out {
        let readability = result.as_ref().ok().map(|report| {
            json!({
                "status": if report.skipped.is_empty() { "passed" } else { "incomplete" },
                "files": report.files, "bytes": report.bytes,
                "skipped": report.skipped.iter().map(|entry| json!({
                    "path": entry.path, "reason": entry.reason,
                })).collect::<Vec<_>>(),
            })
        });
        writeln!(
            out,
            "],\"summary\":{},\"status\":{},\"readability\":{},\"error\":{}}}",
            serde_json::to_string(&summary)?,
            serde_json::to_string(status)?,
            serde_json::to_string(&readability)?,
            serde_json::to_string(&result.as_ref().err().map(|e| error_message(e.as_ref())))?
        )?;
    } else {
        if summary.unavailable > 0 {
            diagnostic(&format!(
                "warning: incomplete integrity verification: {} digests/MACs checked; {} checks unavailable.",
                summary.digests + summary.macs,
                summary.unavailable
            ));
            for ((layer, reason), count) in reasons {
                diagnostic(&format!("  {layer}: {count} checks unavailable ({reason})"));
            }
        }
        let report = result?;
        if !report.skipped.is_empty() {
            diagnostic(&format!(
                "Incomplete: {} files checked; {} entries skipped.",
                report.files,
                report.skipped.len()
            ));
        }
        if !cli.quiet {
            if summary.unavailable == 0 {
                diagnostic(&format!(
                    "Stored integrity passed: {} SHA-256 digests and {} PFS HMACs.",
                    summary.digests, summary.macs
                ));
            }
            diagnostic(&format!(
                "Checked {} files ({}). Authenticity was not checked.",
                report.files,
                human_size(report.bytes)
            ));
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cancelled_json_is_closed_and_returns_130() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut bytes = vec![0; 4096];
        bytes[..4].copy_from_slice(b"\x7fCNT");
        std::fs::write(file.path(), bytes).unwrap();
        let cli = Cli::parse_from([
            "orbis",
            "verify",
            "--integrity",
            "--json",
            "--metadata-only",
            file.path().to_str().unwrap(),
        ]);
        let mut output = Vec::new();
        let code = super::super::run(&cli, &mut output, Arc::new(AtomicBool::new(true))).unwrap();
        assert_eq!(code, 130);
        let report: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(report["status"], "interrupted");
        assert_eq!(report["checks"].as_array().unwrap().len(), 0);
        assert!(report["error"].is_string());
    }

    #[test]
    fn json_output_errors_propagate() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let cli = Cli::parse_from(["orbis", "verify", "missing.pkg", "--integrity", "--json"]);
        let error =
            super::super::run(&cli, &mut Closed, Arc::new(AtomicBool::new(false))).unwrap_err();
        assert!(has_io_kind(error.as_ref(), io::ErrorKind::BrokenPipe));
    }
}
