use super::super::directory::{self, DirectoryIssueKind};
use super::*;
use crate::inventory::hash_file;

pub(super) fn run(
    prepared: &PreparedExport,
    destination: &ExportDestination,
    progress: &impl ExtractProgress,
) -> Result<(Dir, directory::ExportManifest)> {
    let (mut manifest, missing, existing_dir) = prepare(prepared, destination, progress)?;
    let ExportDestination {
        parent,
        parent_path,
        name,
    } = destination;
    super::super::FilesystemPlan::inspect(
        parent_path,
        manifest
            .files
            .iter()
            .filter(|f| missing.contains(&f.path))
            .map(|f| f.bytes),
        prepared.entries.len(),
    )?
    .require_space()?;
    let dir = match existing_dir {
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

fn prepare(
    prepared: &PreparedExport,
    destination: &ExportDestination,
    progress: &impl ExtractProgress,
) -> Result<(
    directory::ExportManifest,
    std::collections::BTreeSet<String>,
    Option<Dir>,
)> {
    progress.phase("Hashing export sources for safe resume");
    progress.bytes_total(prepared.report.bytes);
    let mut files = Vec::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    for entry in prepared.files() {
        let (source, expected) = prepared.source(entry)?;
        let record = hash_file(source, &entry.path, &mut buffer, progress)?;
        if record.bytes != entry.bytes || expected.is_some_and(|expected| expected != &record) {
            return Err(invalid(format!(
                "source changed while planning resume: {}",
                entry.path
            )));
        }
        files.push(record);
    }
    let manifest = prepared.manifest(files, false);
    let ExportDestination {
        parent,
        parent_path: _,
        name,
    } = destination;
    let existing = storage::exists(parent, name)?;
    let mut missing: std::collections::BTreeSet<String> =
        manifest.files.iter().map(|f| f.path.clone()).collect();
    let existing_dir = if existing {
        storage::require_directory(parent, name)?;
        let dir = parent.open_dir(name)?;
        let mut previous = directory::read_manifest(&dir)?;
        previous.complete = false;
        if previous != manifest {
            return Err(invalid(
                "resume sources or manifest changed; choose a new output directory",
            ));
        }
        let verification = directory::verify_open_directory(&dir, &manifest, progress)?;
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
        missing = verification
            .issues
            .into_iter()
            .filter(|i| i.kind == DirectoryIssueKind::Missing)
            .map(|i| i.path)
            .collect();
        Some(dir)
    } else {
        None
    };
    Ok((manifest, missing, existing_dir))
}

pub(super) fn preview(
    prepared: &PreparedExport,
    destination: &ExportDestination,
    progress: &impl ExtractProgress,
) -> Result<super::super::FilesystemPlan> {
    let (manifest, missing, _) = prepare(prepared, destination, progress)?;
    super::super::FilesystemPlan::inspect(
        &destination.parent_path,
        manifest
            .files
            .iter()
            .filter(|f| missing.contains(&f.path))
            .map(|f| f.bytes),
        prepared.entries.len(),
    )
}
