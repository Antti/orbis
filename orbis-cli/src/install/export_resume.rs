use super::super::directory::{self, DirectoryIssueKind};
use super::*;
use std::collections::BTreeSet;

pub(super) fn run(
    prepared: &PreparedExport,
    destination: &ExportDestination,
    mut manifest: directory::ExportManifest,
    progress: &impl ExtractProgress,
) -> Result<(Dir, directory::ExportManifest)> {
    let remaining = inspect_remaining(&manifest, destination, progress)?;
    remaining.filesystem.require_space()?;
    let missing = remaining.missing;
    let parent = &destination.parent;
    let name = &destination.name;
    let dir = match remaining.directory {
        Some(dir) => dir,
        None => {
            parent.create_dir(name)?;
            parent.open_dir(name)?
        }
    };
    // Record intent before exposing any game files. A crash before this write
    // leaves an unowned empty directory that resume refuses to adopt.
    storage::write_json(&dir, directory::MANIFEST, &manifest)?;
    for path in &manifest.directories {
        dir.create_dir_all(path)?;
    }
    progress.phase("Resuming merged export");
    progress.bytes_total(
        manifest
            .files
            .iter()
            .filter(|f| missing.contains(&f.path))
            .map(|f| f.bytes)
            .sum(),
    );
    for record in &manifest.files {
        check_cancelled(progress)?;
        if !missing.contains(&record.path) {
            continue;
        }
        let entry = &prepared.entries[&record.path.to_ascii_lowercase()];
        let (source, _) = prepared.source(entry)?;
        let temporary = format!(".orbis-resume-{:032x}.tmp", fastrand::u128(..));
        let result = (|| {
            copy_file(source, &dir, entry, Some(record), &temporary, progress)?;
            crate::fs_output::publish_new_file(&dir, &temporary, Path::new(&record.path), || {
                if progress.is_cancelled() {
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(())
                }
            })
            .map_err(|e| {
                if e.kind() == io::ErrorKind::Interrupted && progress.is_cancelled() {
                    super::super::InstallError::from(ExtractError::Cancelled)
                } else {
                    e.into()
                }
            })
        })();
        let _ = dir.remove_file(&temporary);
        result?;
    }
    for path in manifest.directories.iter().rev() {
        storage::sync_dir(&dir.open_dir(path)?)?;
    }
    check_cancelled(progress)?;
    manifest.complete = true;
    storage::write_json(&dir, directory::MANIFEST, &manifest)?;
    storage::sync_dir(parent)?;
    Ok((dir, manifest))
}

pub(super) struct RemainingExport {
    pub filesystem: super::super::FilesystemPlan,
    directory: Option<Dir>,
    missing: BTreeSet<String>,
}

impl RemainingExport {
    pub fn work(&self, manifest: &directory::ExportManifest) -> ResumePlan {
        let mut remaining_files = 0;
        let mut remaining_bytes = 0;
        for file in &manifest.files {
            if self.missing.contains(&file.path) {
                remaining_files += 1;
                remaining_bytes += file.bytes;
            }
        }
        ResumePlan {
            reusable_files: manifest.files.len() - remaining_files,
            remaining_files,
            remaining_bytes,
        }
    }
}

pub(super) fn inspect_remaining(
    manifest: &directory::ExportManifest,
    destination: &ExportDestination,
    progress: &impl ExtractProgress,
) -> Result<RemainingExport> {
    let ExportDestination { parent, name, .. } = destination;
    let (existing_dir, missing): (_, BTreeSet<String>) = if storage::exists(parent, name)? {
        storage::require_directory(parent, name)?;
        let dir = parent.open_dir(name)?;
        let mut previous = directory::read_manifest(&dir)?;
        previous.complete = false;
        if &previous != manifest {
            return Err(invalid(
                "resume sources or manifest changed; choose a new output directory",
            ));
        }
        let verification = directory::verify_open_directory(&dir, manifest, progress)?;
        if let Some(issue) = verification
            .issues
            .iter()
            .find(|i| i.kind != DirectoryIssueKind::Missing)
        {
            return Err(invalid(format!(
                "resume refuses {} file {}; preserve or remove the conflict before retrying",
                issue.kind, issue.path
            )));
        }
        let missing = verification
            .issues
            .into_iter()
            .filter(|i| i.kind == DirectoryIssueKind::Missing)
            .map(|i| i.path)
            .collect();
        (Some(dir), missing)
    } else {
        (
            None,
            manifest.files.iter().map(|f| f.path.clone()).collect(),
        )
    };
    let volume = if existing_dir.is_some() {
        destination.path()
    } else {
        destination.parent_path.clone()
    };
    let filesystem = super::super::FilesystemPlan::inspect(
        &volume,
        manifest
            .files
            .iter()
            .filter(|f| missing.contains(&f.path))
            .map(|f| f.bytes),
        manifest.files.len() + manifest.directories.len(),
    )?;
    Ok(RemainingExport {
        filesystem,
        missing,
        directory: existing_dir,
    })
}
