use super::*;
use orbis_integrity::Status;
use serde::Serialize;
use std::{collections::BTreeMap, io::Write, time::SystemTime};

#[derive(Debug, Default, Serialize)]
pub struct CheckSummary {
    pub digests: usize,
    pub macs: usize,
    pub unavailable: usize,
    pub failed: usize,
}

#[derive(Debug, Serialize)]
pub struct Readability {
    pub files: usize,
    pub bytes: u64,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Serialize)]
pub struct Skipped {
    path: String,
    reason: String,
}

/// Summary-only report: block checks are counted, never retained in memory.
#[derive(Debug, Serialize)]
pub struct VerificationReport {
    pub schema_version: u32,
    pub report_type: &'static str,
    pub package: String,
    pub started_at_unix_seconds: u64,
    pub metadata_only: bool,
    pub status: &'static str,
    pub summary: CheckSummary,
    pub unavailable_reasons: BTreeMap<String, usize>,
    pub readability: Option<Readability>,
    pub error: Option<String>,
    pub authenticity_checked: bool,
}

impl VerificationReport {
    pub fn title(&self) -> &'static str {
        match self.status {
            "complete" if self.metadata_only => "Metadata verification passed",
            "complete" => "Full verification passed",
            "incomplete" => "Verification incomplete",
            "interrupted" => "Verification cancelled",
            _ => "Verification failed",
        }
    }

    /// Publish a new report atomically; never overwrite a PKG or existing file.
    pub fn save(&self, destination: &Path) -> Result<()> {
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist_noclobber(destination)?;
        Ok(())
    }
}

pub(super) fn verify(
    path: &Path,
    metadata_only: bool,
    passcode: Option<&Passcode>,
    progress: &impl ExtractProgress,
) -> VerificationReport {
    let mut report = VerificationReport {
        schema_version: 1,
        report_type: "orbis-desktop-verification-summary",
        package: path.to_string_lossy().into_owned(),
        started_at_unix_seconds: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        metadata_only,
        status: "failed",
        summary: CheckSummary::default(),
        unavailable_reasons: BTreeMap::new(),
        readability: None,
        error: None,
        authenticity_checked: false,
    };
    let result = (|| -> Result<_> {
        if progress.is_cancelled() {
            return Err(ExtractError::Cancelled.into());
        }
        let mut pkg = unsafe { orbis_cli::inspect_pkg(path)? };
        if let Some(passcode) = passcode {
            pkg.load_passcode(passcode.bytes())?;
        }
        let _ = pkg.load_entry_keys();
        if !metadata_only {
            let _ = pkg.load_pfs_key();
        }
        let extractor = PkgExtractor::new(&pkg, progress, false);
        extractor.verify_integrity_with(metadata_only, |layer, check| {
            match check.status {
                Status::Passed if layer == "pkg" => report.summary.digests += check.count,
                Status::Passed => report.summary.macs += check.count,
                Status::Unavailable => {
                    report.summary.unavailable += check.count;
                    let reason = check.reason.map_or("unknown", |r| r.as_str());
                    *report
                        .unavailable_reasons
                        .entry(format!("{layer}: {reason}"))
                        .or_default() += check.count;
                }
                Status::Failed => report.summary.failed += check.count,
                Status::NotApplicable => {}
            }
            Ok(())
        })?;
        Ok(extractor.verify(&ExtractOptions {
            metadata_only,
            paths: Vec::new(),
        })?)
    })();
    match result {
        Ok(readability) => {
            report.status = if report.summary.unavailable == 0 && readability.skipped.is_empty() {
                "complete"
            } else {
                "incomplete"
            };
            report.readability = Some(Readability {
                files: readability.files,
                bytes: readability.bytes,
                skipped: readability
                    .skipped
                    .into_iter()
                    .map(|s| Skipped {
                        path: s.path,
                        reason: s.reason,
                    })
                    .collect(),
            });
        }
        Err(error) => {
            let failure = Failure::from_error(error.as_ref());
            report.status = if failure.cancelled {
                "interrupted"
            } else {
                "failed"
            };
            report.error = Some(failure.details);
        }
    }
    report
}
