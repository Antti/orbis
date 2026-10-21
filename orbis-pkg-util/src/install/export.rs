//! Materialize the installed base/update overlay as an independent directory.
use super::{Result, invalid, storage};
use crate::{
    ExtractError, ExtractProgress,
    fs_output::{copy_new_directory, publish_new, unsupported_publication},
};
use cap_std::{ambient_authority, fs::Dir, fs::OpenOptions};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[path = "export_resume.rs"]
mod resume;

/// The exported version and the bytes actually copied (overridden base files are skipped).
#[derive(Debug, serde::Serialize)]
pub struct ExportReport {
    pub title_id: String,
    pub app_version: String,
    pub destination: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub update_files: usize,
}

/// A read-only snapshot. Revalidated when export starts.
#[derive(Debug, serde::Serialize)]
pub struct ExportPlan {
    pub schema_version: u32,
    pub title_id: String,
    pub base_version: String,
    pub update_version: Option<String>,
    pub destination: PathBuf,
    pub files: usize,
    pub update_files: usize,
    pub bytes: u64,
    pub filesystem: super::FilesystemPlan,
}

pub fn plan_export(
    base: &Path,
    output: &Path,
    progress: impl ExtractProgress,
) -> Result<ExportPlan> {
    let location = storage::Layout::open(base, false)?;
    let (_, parent, name) = destination(&location, output)?;
    location.require_recovered()?;
    let metadata = location.base_metadata()?;
    let manifest = location.active_manifest(&location.state()?, &metadata, &progress)?;
    let mut entries = BTreeMap::new();
    scan(&location.base_dir()?, Layer::Base, &mut entries, &progress)?;
    if manifest.is_some() {
        scan(
            &location.parent.open_dir(&location.active)?,
            Layer::Update,
            &mut entries,
            &progress,
        )?;
    }
    let files: Vec<_> = entries.values().filter(|e| !e.directory).collect();
    let bytes = files
        .iter()
        .try_fold(0u64, |n, e| n.checked_add(e.bytes))
        .ok_or_else(|| invalid("export size overflows"))?;
    Ok(ExportPlan {
        schema_version: 1,
        title_id: metadata.title_id,
        base_version: metadata.version.to_string(),
        update_version: manifest.map(|m| m.app_version),
        destination: parent.join(name),
        files: files.len(),
        update_files: files.iter().filter(|e| e.layer == Layer::Update).count(),
        bytes,
        filesystem: super::FilesystemPlan::inspect(
            &parent,
            files.iter().map(|e| e.bytes),
            entries.len(),
        )?,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Base,
    Update,
}

struct Entry {
    path: String,
    layer: Layer,
    directory: bool,
    bytes: u64,
}

/// Copies an original base plus its managed installed update into a new directory.
///
/// Update files win at matching paths. With no installed update, copies the base.
/// Validates the managed update, excludes installer metadata, and holds the install
/// lock throughout. The output parent must already exist and must be outside the
/// base, update, and installation state. Existing destinations are never replaced.
/// Symlinks, special files, case collisions, and file/directory conflicts are rejected.
///
/// All content is copied, not linked. Filesystems without exclusive rename use a
/// second copy into an exclusively created destination, visible while copying.
/// Errors or cooperative cancellation discard staging and attempt to remove owned
/// fallback output. A forced exit can leave staging and a partial destination.
/// Callers must keep the source files unchanged during export. This does not decrypt
/// executables, apply binary deltas, or establish that a game will run in a consumer.
pub fn export_merged<P: ExtractProgress>(
    base: &Path,
    output: &Path,
    progress: P,
) -> Result<ExportReport> {
    export_impl(base, output, progress, false)
}

/// Export directly into a resumable directory. The output is visible while
/// incomplete; rerun with resume to verify completed files and copy missing ones.
pub fn resume_export<P: ExtractProgress>(
    base: &Path,
    output: &Path,
    progress: P,
) -> Result<ExportReport> {
    export_impl(base, output, progress, true)
}

fn export_impl<P: ExtractProgress>(
    base: &Path,
    output: &Path,
    progress: P,
    resume: bool,
) -> Result<ExportReport> {
    check_cancelled(&progress)?;
    // Validate the destination before creating the state directory/lock.
    let preview = storage::Layout::open(base, false)?;
    let (parent, parent_path, destination) = destination_inner(&preview, output, resume)?;
    drop(preview);
    let location = storage::Layout::open(base, true)?;
    location.require_recovered()?;
    let metadata = location.base_metadata()?;
    let state = location.state()?;
    let manifest = location.active_manifest(&state, &metadata, &progress)?;
    let base_dir = location.base_dir()?;
    let update_dir = if manifest.is_some() {
        Some(location.parent.open_dir(&location.active)?)
    } else {
        None
    };

    progress.phase("Planning merged export");
    let mut entries = BTreeMap::new();
    scan(&base_dir, Layer::Base, &mut entries, &progress)?;
    if let Some(dir) = &update_dir {
        scan(dir, Layer::Update, &mut entries, &progress)?;
    }
    let mut report = ExportReport {
        title_id: metadata.title_id.clone(),
        app_version: manifest
            .as_ref()
            .map_or_else(|| metadata.version.to_string(), |m| m.app_version.clone()),
        destination: parent_path.join(&destination),
        files: 0,
        bytes: 0,
        update_files: 0,
    };
    for entry in entries.values().filter(|entry| !entry.directory) {
        report.files += 1;
        report.update_files += usize::from(entry.layer == Layer::Update);
        report.bytes = report
            .bytes
            .checked_add(entry.bytes)
            .ok_or_else(|| invalid("export size overflows"))?;
    }
    let expected: BTreeMap<_, _> = manifest
        .iter()
        .flat_map(|m| m.files.iter().map(|file| (file.path.as_str(), file)))
        .collect();
    if expected.len() != report.update_files {
        return Err(invalid("update changed during export"));
    }
    if resume {
        resume::run(
            (&base_dir, update_dir.as_ref()),
            &metadata,
            manifest.as_ref(),
            &entries,
            &report,
            &progress,
        )?;
        return Ok(report);
    }
    super::FilesystemPlan::inspect(
        &parent_path,
        entries.values().filter(|e| !e.directory).map(|e| e.bytes),
        entries.len(),
    )?
    .require_space()?;
    let stage = Stage::new(parent, destination)?;
    progress.phase("Exporting merged game");
    progress.bytes_total(report.bytes);
    let mut records = Vec::new();
    for entry in entries.values() {
        check_cancelled(&progress)?;
        if entry.directory {
            stage.dir.as_ref().unwrap().create_dir_all(&entry.path)?;
            continue;
        }
        let source = match entry.layer {
            Layer::Base => &base_dir,
            Layer::Update => update_dir.as_ref().unwrap(),
        };
        let record = if entry.layer == Layer::Update {
            Some(
                *expected
                    .get(entry.path.as_str())
                    .ok_or_else(|| invalid("update changed during export"))?,
            )
        } else {
            None
        };
        records.push(copy_file(
            source,
            stage.dir.as_ref().unwrap(),
            entry,
            record,
            &entry.path,
            &progress,
        )?);
    }
    let export_manifest = super::directory::ExportManifest {
        schema_version: 1,
        title_id: metadata.title_id,
        content_id: metadata.content_id,
        base_version: metadata.version.to_string(),
        update_version: manifest.as_ref().map(|m| m.app_version.clone()),
        complete: true,
        directories: entries
            .values()
            .filter(|e| e.directory)
            .map(|e| e.path.clone())
            .collect(),
        files: records,
    };
    storage::write_json(
        stage.dir.as_ref().unwrap(),
        super::directory::MANIFEST,
        &export_manifest,
    )?;
    entries.insert(
        super::directory::MANIFEST.into(),
        Entry {
            path: super::directory::MANIFEST.into(),
            layer: Layer::Base,
            directory: false,
            bytes: 0,
        },
    );
    // Persist all newly created directories before publishing their common root.
    for entry in entries.values().rev().filter(|entry| entry.directory) {
        check_cancelled(&progress)?;
        storage::sync_dir(&stage.dir.as_ref().unwrap().open_dir(&entry.path)?)?;
    }
    check_cancelled(&progress)?;
    stage.publish(&entries, report.bytes, &progress)?;
    progress.pfs_completed();
    Ok(report)
}

fn check_cancelled(progress: &impl ExtractProgress) -> Result<()> {
    if progress.is_cancelled() {
        return Err(ExtractError::Cancelled.into());
    }
    Ok(())
}

fn destination(location: &storage::Layout, output: &Path) -> Result<(Dir, PathBuf, OsString)> {
    destination_inner(location, output, false)
}
fn destination_inner(
    location: &storage::Layout,
    output: &Path,
    allow_existing: bool,
) -> Result<(Dir, PathBuf, OsString)> {
    let name = output
        .file_name()
        .ok_or_else(|| invalid("export output must name a new directory"))?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = std::fs::canonicalize(parent).map_err(|e| {
        invalid(format!(
            "cannot open export parent directory (it must already exist): {e}"
        ))
    })?;
    let path = parent.join(name);
    for source in [&location.base, &location.active, &location.root_name] {
        if path.starts_with(location.parent_path.join(source)) {
            return Err(invalid(
                "export output must be outside the base, update, and installation state",
            ));
        }
    }
    let dir = Dir::open_ambient_dir(&parent, ambient_authority())?;
    // realpath/canonicalize can preserve an alias's capitalization on macOS.
    // Compare directory identities too, so aliases cannot hide a nested output.
    #[cfg(any(unix, windows))]
    {
        let mut sources = Vec::new();
        for source in [&location.base, &location.active, &location.root_name] {
            if storage::exists(&location.parent, source)? {
                sources.push(directory_id(&location.parent.open_dir(source)?)?);
            }
        }
        for ancestor in parent.ancestors() {
            let ancestor = Dir::open_ambient_dir(ancestor, ambient_authority())?;
            if sources.contains(&directory_id(&ancestor)?) {
                return Err(invalid(
                    "export output must be outside the base, update, and installation state",
                ));
            }
        }
    }
    if !allow_existing && storage::exists(&dir, name)? {
        return Err(invalid(
            "export output already exists; choose a new --output directory",
        ));
    }
    Ok((dir, parent, name.to_owned()))
}

#[cfg(unix)]
fn directory_id(dir: &Dir) -> Result<(u64, u64)> {
    use cap_std::fs::MetadataExt;
    let meta = dir.dir_metadata()?;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn directory_id(dir: &Dir) -> Result<(u64, u64)> {
    use std::{mem::MaybeUninit, os::windows::io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // The directory handle remains live and the API initializes info on success.
    if unsafe { GetFileInformationByHandle(dir.as_raw_handle(), info.as_mut_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let info = unsafe { info.assume_init() };
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

fn scan(
    root: &Dir,
    layer: Layer,
    entries: &mut BTreeMap<String, Entry>,
    progress: &impl ExtractProgress,
) -> Result<()> {
    // Queue relative paths rather than open directories to bound descriptor usage.
    let mut pending = vec![String::new()];
    while let Some(prefix) = pending.pop() {
        check_cancelled(progress)?;
        let path = if prefix.is_empty() {
            "."
        } else {
            prefix.trim_end_matches('/')
        };
        storage::require_directory(root, path)?;
        let dir = root.open_dir(path)?;
        for entry in dir.entries()? {
            check_cancelled(progress)?;
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("non-UTF8 export filename"))?;
            let path = format!("{prefix}{name}");
            if path == storage::MANIFEST
                || path == super::directory::MANIFEST
                || super::directory::apple_double(&dir, &name)?
            {
                continue;
            }
            storage::validate_path(&path)?;
            let meta = dir.symlink_metadata(&name)?;
            if !meta.is_dir() && !meta.is_file() {
                return Err(invalid(format!(
                    "symlinks and special files cannot be exported: {path}"
                )));
            }
            let key = path.to_ascii_lowercase();
            if let Some(previous) = entries.get(&key) {
                if previous.path != path || previous.layer == layer {
                    return Err(invalid(format!("case-colliding export path: {path}")));
                }
                if previous.directory != meta.is_dir() {
                    return Err(invalid(format!("export file/directory conflict: {path}")));
                }
            }
            if meta.is_dir() {
                pending.push(format!("{path}/"));
            }
            entries.insert(
                key,
                Entry {
                    path,
                    layer,
                    directory: meta.is_dir(),
                    bytes: meta.len(),
                },
            );
        }
    }
    Ok(())
}

fn copy_file(
    source: &Dir,
    destination: &Dir,
    entry: &Entry,
    expected: Option<&storage::FileRecord>,
    target_path: &str,
    progress: &impl ExtractProgress,
) -> Result<storage::FileRecord> {
    if !source.symlink_metadata(&entry.path)?.is_file() {
        return Err(invalid(format!(
            "source is no longer a regular file: {}",
            entry.path
        )));
    }
    let mut input = source.open(&entry.path)?;
    let before = input.metadata()?;
    if !before.is_file() || before.len() != entry.bytes {
        return Err(invalid(format!(
            "source changed during export: {}",
            entry.path
        )));
    }
    let mut buffer = vec![0; 1024 * 1024];
    let mut output =
        destination.open_with(target_path, OpenOptions::new().write(true).create_new(true))?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    loop {
        check_cancelled(progress)?;
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > entry.bytes {
            return Err(invalid(format!(
                "source grew during export: {}",
                entry.path
            )));
        }
        output.write_all(&buffer[..n])?;
        hash.update(&buffer[..n]);
        progress.bytes_processed(n as u64);
    }
    let after = input.metadata()?;
    if bytes != entry.bytes || before.modified()? != after.modified()? {
        return Err(invalid(format!(
            "source changed during export: {}",
            entry.path
        )));
    }
    let sha256 = format!("{:x}", hash.finalize());
    if let Some(record) = expected
        && (bytes != record.bytes || sha256 != record.sha256)
    {
        return Err(invalid(format!(
            "update changed during export: {}",
            entry.path
        )));
    }
    output.sync_all()?;
    Ok(storage::FileRecord {
        path: entry.path.clone(),
        bytes,
        sha256,
    })
}

struct Stage {
    parent: Dir,
    dir: Option<Dir>,
    temporary: String,
    destination: OsString,
    identity: Option<(u64, u64)>,
}

impl Stage {
    fn new(parent: Dir, destination: OsString) -> Result<Self> {
        let temporary = format!(".orbis-export-{:032x}.tmp", fastrand::u128(..));
        parent.create_dir(&temporary)?;
        let mut stage = Self {
            parent,
            dir: None,
            temporary,
            destination,
            identity: None,
        };
        stage.dir = Some(stage.parent.open_dir(&stage.temporary)?);
        #[cfg(any(unix, windows))]
        {
            stage.identity = Some(directory_id(stage.dir.as_ref().unwrap())?);
        }
        Ok(stage)
    }

    fn publish(
        self,
        entries: &BTreeMap<String, Entry>,
        bytes: u64,
        progress: &impl ExtractProgress,
    ) -> Result<()> {
        self.publish_with(entries, bytes, progress, publish_new)
    }

    fn publish_with(
        mut self,
        entries: &BTreeMap<String, Entry>,
        bytes: u64,
        progress: &impl ExtractProgress,
        rename: impl FnOnce(&Dir, &str, &Path) -> std::io::Result<()>,
    ) -> Result<()> {
        storage::sync_dir(self.dir.as_ref().unwrap())?;
        // Windows cannot rename a directory while it has an open directory handle.
        self.dir.take();
        match rename(&self.parent, &self.temporary, Path::new(&self.destination)) {
            Err(error) if unsupported_publication(&error) => {
                progress.phase("Publishing export on this filesystem");
                progress.bytes_total(bytes);
                copy_new_directory(
                    &self.parent,
                    &self.temporary,
                    Path::new(&self.destination),
                    entries
                        .values()
                        .map(|entry| (Path::new(&entry.path), entry.directory)),
                    |bytes| {
                        if progress.is_cancelled() {
                            return Err(std::io::ErrorKind::Interrupted.into());
                        }
                        progress.bytes_processed(bytes);
                        Ok(())
                    },
                )
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::Interrupted && progress.is_cancelled() {
                        super::InstallError::from(ExtractError::Cancelled)
                    } else {
                        error.into()
                    }
                })?;
            }
            result => result?,
        }
        if let Err(error) = storage::sync_dir(&self.parent) {
            progress.warning(&format!(
                "Export completed, but syncing its parent directory failed: {error}"
            ));
        }
        Ok(())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        self.dir.take();
        let _ = remove_staging(&self.parent, Path::new(&self.temporary), self.identity);
    }
}

fn remove_staging(parent: &Dir, path: &Path, identity: Option<(u64, u64)>) -> io::Result<()> {
    if !parent.symlink_metadata(path)?.is_dir() {
        return Ok(());
    }
    let root = match parent.open_dir(path) {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    // Do not recurse into a replacement directory after the original was moved.
    #[cfg(any(unix, windows))]
    if identity != Some(directory_id(&root).map_err(io::Error::other)?) {
        return Ok(());
    }
    #[cfg(not(any(unix, windows)))]
    let _ = identity;
    // exFAT on macOS removes AppleDouble sidecars alongside their associated
    // files. A previously enumerated sidecar may therefore disappear before
    // unlink. Tolerate those missing entries at every level of this private tree.
    let mut pending = vec![(PathBuf::new(), false)];
    while let Some((relative, visited)) = pending.pop() {
        let result = (|| {
            if visited {
                return root.remove_dir(&relative);
            }
            let dir = root.open_dir(if relative.as_os_str().is_empty() {
                Path::new(".")
            } else {
                &relative
            })?;
            if !relative.as_os_str().is_empty() {
                pending.push((relative.clone(), true));
            }
            for entry in dir.entries()? {
                let entry = match entry {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    entry => entry?,
                };
                let child = relative.join(entry.file_name());
                match root.symlink_metadata(&child) {
                    Ok(metadata) if metadata.is_dir() => pending.push((child, false)),
                    Ok(_) => match root.remove_file(&child) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                        _ => {}
                    },
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        })();
        if let Err(error) = result
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(error);
        }
    }
    drop(root);
    parent.remove_dir(path)
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
