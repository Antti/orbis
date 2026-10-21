use super::super::directory::{self, ExportManifest, hash_file};
use super::*;
use orbis_pkg::sfo::AppMetadata;

pub(super) fn run(
    sources: (&Dir, Option<&Dir>),
    metadata: &AppMetadata,
    update: Option<&storage::Manifest>,
    entries: &BTreeMap<String, Entry>,
    report: &ExportReport,
    progress: &impl ExtractProgress,
) -> Result<()> {
    progress.phase("Hashing export sources for safe resume");
    progress.bytes_total(report.bytes);
    let mut files = Vec::new();
    for entry in entries.values().filter(|e| !e.directory) {
        let source = if entry.layer == Layer::Base {
            sources.0
        } else {
            sources.1.unwrap()
        };
        let record = hash_file(source, &entry.path, progress)?;
        if record.bytes != entry.bytes
            || (entry.layer == Layer::Update && !update.unwrap().files.contains(&record))
        {
            return Err(invalid(format!(
                "source changed while planning resume: {}",
                entry.path
            )));
        }
        files.push(record);
    }
    let mut manifest = ExportManifest {
        schema_version: 1,
        title_id: metadata.title_id.clone(),
        content_id: metadata.content_id.clone(),
        base_version: metadata.version.to_string(),
        update_version: update.map(|m| m.app_version.clone()),
        complete: false,
        directories: entries
            .values()
            .filter(|e| e.directory)
            .map(|e| e.path.clone())
            .collect(),
        files,
    };
    let output = &report.destination;
    let parent_path = output.parent().unwrap();
    let parent = Dir::open_ambient_dir(parent_path, ambient_authority())?;
    let name = output.file_name().unwrap();
    let existing = storage::exists(&parent, name)?;
    let mut missing: std::collections::BTreeSet<String> =
        manifest.files.iter().map(|f| f.path.clone()).collect();
    if existing {
        storage::require_directory(&parent, name)?;
        let dir = parent.open_dir(name)?;
        let mut previous = directory::read_manifest(&dir)?;
        previous.complete = false;
        if previous != manifest {
            return Err(invalid(
                "resume sources or manifest changed; choose a new output directory",
            ));
        }
        let verification = directory::verify_directory(output, progress)?;
        if let Some(issue) = verification.issues.iter().find(|i| i.kind != "missing") {
            return Err(invalid(format!(
                "resume refuses {} file {}; preserve or remove the conflict before retrying",
                issue.kind, issue.path
            )));
        }
        missing = verification
            .issues
            .into_iter()
            .filter(|i| i.kind == "missing")
            .map(|i| i.path)
            .collect();
    }
    super::super::FilesystemPlan::inspect(
        parent_path,
        manifest
            .files
            .iter()
            .filter(|f| missing.contains(&f.path))
            .map(|f| f.bytes),
        entries.len(),
    )?
    .require_space()?;
    if !existing {
        parent.create_dir(name)?;
    }
    let dir = parent.open_dir(name)?;
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
        let entry = &entries[&record.path.to_ascii_lowercase()];
        let source = if entry.layer == Layer::Base {
            sources.0
        } else {
            sources.1.unwrap()
        };
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
    storage::sync_dir(&parent)?;
    Ok(())
}
