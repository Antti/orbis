//! Materialize the installed base/update overlay as an independent directory.
use super::{Result, invalid, storage};
use crate::{ExtractError, ExtractProgress, fs_output::publish_new};
use cap_std::{ambient_authority, fs::Dir, fs::OpenOptions};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// The exported version and the bytes actually copied (overridden base files are skipped).
#[derive(Debug)]
pub struct ExportReport {
    pub title_id: String,
    pub app_version: String,
    pub destination: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub update_files: usize,
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
/// All content is copied, not linked. Errors or cooperative cancellation discard the
/// staging directory; a forced process exit may leave a `.orbis-export-*.tmp` sibling.
/// Callers must keep the source files unchanged during export. This does not decrypt
/// executables, apply binary deltas, or establish that a game will run in a consumer.
pub fn export_merged<P: ExtractProgress>(
    base: &Path,
    output: &Path,
    progress: P,
) -> Result<ExportReport> {
    check_cancelled(&progress)?;
    // Validate the destination before creating the state directory/lock.
    let preview = storage::Layout::open(base, false)?;
    let (parent, parent_path, destination) = destination(&preview, output)?;
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
        title_id: metadata.title_id,
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
    let stage = Stage::new(parent, destination)?;
    progress.phase("Exporting merged game");
    progress.bytes_total(report.bytes);
    let mut buffer = vec![0; 1024 * 1024];
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
        copy_file(
            source,
            stage.dir.as_ref().unwrap(),
            entry,
            record,
            &mut buffer,
            &progress,
        )?;
    }
    // Persist all newly created directories before publishing their common root.
    for entry in entries.values().rev().filter(|entry| entry.directory) {
        check_cancelled(&progress)?;
        storage::sync_dir(&stage.dir.as_ref().unwrap().open_dir(&entry.path)?)?;
    }
    check_cancelled(&progress)?;
    stage.publish(&progress)?;
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
    if storage::exists(&dir, name)? {
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
            if path == storage::MANIFEST {
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
    buffer: &mut [u8],
    progress: &impl ExtractProgress,
) -> Result<()> {
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
    let mut output =
        destination.open_with(&entry.path, OpenOptions::new().write(true).create_new(true))?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    loop {
        check_cancelled(progress)?;
        let n = input.read(buffer)?;
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
        if expected.is_some() {
            hash.update(&buffer[..n]);
        }
        progress.bytes_processed(n as u64);
    }
    let after = input.metadata()?;
    if bytes != entry.bytes || before.modified()? != after.modified()? {
        return Err(invalid(format!(
            "source changed during export: {}",
            entry.path
        )));
    }
    if let Some(record) = expected
        && (bytes != record.bytes || format!("{:x}", hash.finalize()) != record.sha256)
    {
        return Err(invalid(format!(
            "update changed during export: {}",
            entry.path
        )));
    }
    output.sync_all()?;
    Ok(())
}

struct Stage {
    parent: Dir,
    dir: Option<Dir>,
    temporary: String,
    destination: OsString,
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
        };
        stage.dir = Some(stage.parent.open_dir(&stage.temporary)?);
        Ok(stage)
    }

    fn publish(mut self, progress: &impl ExtractProgress) -> Result<()> {
        storage::sync_dir(self.dir.as_ref().unwrap())?;
        // Windows cannot rename a directory while it has an open directory handle.
        self.dir.take();
        publish_new(&self.parent, &self.temporary, Path::new(&self.destination))?;
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
        let _ = self.parent.remove_dir_all(&self.temporary);
    }
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
