use super::super::directory::{self, DirectoryReport, ExportManifest};
use super::*;

/// A completed export, retaining its directory and the manifest built from the
/// copied sources. Verification does not trust a replacement directory or a
/// rewritten manifest found at the destination path.
#[derive(Debug)]
pub struct ExportedDirectory {
    report: ExportReport,
    dir: Dir,
    manifest: ExportManifest,
}

impl ExportedDirectory {
    pub(super) fn new(report: ExportReport, dir: Dir, manifest: ExportManifest) -> Self {
        Self {
            report,
            dir,
            manifest,
        }
    }

    pub fn report(&self) -> &ExportReport {
        &self.report
    }

    pub fn into_report(self) -> ExportReport {
        self.report
    }

    /// Verify the retained directory against this export's expected hashes.
    /// Check the displayed path and persisted manifest before and after hashing
    /// so a concurrent replacement cannot be reported as a verified export.
    pub fn verify(&self, progress: impl ExtractProgress) -> Result<DirectoryReport> {
        self.check_destination()?;
        let checked = directory::verify_open_directory(&self.dir, &self.manifest, &progress)?;
        self.check_destination()?;
        Ok(checked)
    }

    fn check_destination(&self) -> Result<()> {
        let path = &self.report.destination;
        if !std::fs::symlink_metadata(path)?.is_dir() {
            return Err(invalid(
                "export destination changed before verification completed",
            ));
        }
        let current = Dir::open_ambient_dir(path, ambient_authority())?;
        if directory_identity(&current)? != directory_identity(&self.dir)? {
            return Err(invalid(
                "export destination changed before verification completed",
            ));
        }
        if directory::read_manifest(&self.dir)? != self.manifest {
            return Err(invalid(
                "export manifest changed before verification completed",
            ));
        }
        Ok(())
    }
}
