use super::ExtractError;
use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};
use std::path::{Component, Path, PathBuf};
use std::{io, io::Write};

/// Stages a file beside its destination, using the same confined directory
/// handle for creation, publication, and cleanup.
pub(super) struct StagedFile {
    parent: Dir,
    temporary: String,
    destination: PathBuf,
    display_path: PathBuf,
    file: Option<File>,
    overwrite: bool,
}

impl Write for StagedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.as_mut().unwrap().write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().unwrap().flush()
    }
}

impl StagedFile {
    pub fn commit(mut self) -> Result<(), ExtractError> {
        self.file
            .take()
            .unwrap()
            .sync_all()
            .map_err(|source| ExtractError::WriteFailed {
                path: self.display_path.clone(),
                source,
            })?;
        let result = if self.overwrite {
            // rename replaces a symlink itself, never its target. Refuse known
            // symlinks anyway so --force retains the extractor's existing policy.
            check_destination(&self.parent, &self.destination, true).and_then(|()| {
                self.parent
                    .rename(&self.temporary, &self.parent, &self.destination)
            })
        } else {
            publish_new(&self.parent, &self.temporary, &self.destination)
        };
        result.map_err(|source| ExtractError::CreateFileFailed {
            path: self.display_path.clone(),
            source,
        })
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        self.file.take();
        let _ = self.parent.remove_file(&self.temporary);
    }
}

fn publish_new(parent: &Dir, temporary: &str, destination: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        rustix::fs::renameat_with(
            parent,
            temporary,
            parent,
            destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        // cap-std's Windows directory handles deny FILE_SHARE_DELETE, keeping
        // the directory stable during path-based operations (as in its rename).
        // Only single filenames, never archive-controlled parents, are joined.
        let directory = parent.try_clone()?.into_std_file();
        let root = winx::file::get_file_path(&directory)?;
        let from: Vec<_> = root
            .join(temporary)
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        let to: Vec<_> = root
            .join(destination)
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        // No REPLACE_EXISTING or COPY_ALLOWED: fail if the destination appears,
        // and rename within this directory without requiring hard-link support.
        let result = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        parent.hard_link(temporary, parent, destination)
    }
}

fn check_destination(dir: &Dir, path: &Path, overwrite: bool) -> io::Result<()> {
    match dir.symlink_metadata(path) {
        Ok(metadata) if overwrite && metadata.is_file() => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination already exists",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// All archive-controlled paths are resolved relative to this directory handle.
/// cap-std prevents symlink traversal and directory-renaming races from escaping it.
pub(super) struct Output {
    dir: Dir,
    pub path: PathBuf,
}

impl Output {
    pub fn new(path: &Path) -> Result<Self, ExtractError> {
        std::fs::create_dir_all(path).map_err(|source| ExtractError::CreateDirectoryFailed {
            path: path.to_owned(),
            source,
        })?;
        let dir = Dir::open_ambient_dir(path, ambient_authority()).map_err(|source| {
            ExtractError::CreateDirectoryFailed {
                path: path.to_owned(),
                source,
            }
        })?;
        Ok(Self {
            dir,
            path: path.to_owned(),
        })
    }

    pub fn create_dir(&self, path: &Path) -> Result<(), ExtractError> {
        self.dir
            .create_dir_all(path)
            .map_err(|source| ExtractError::CreateDirectoryFailed {
                path: self.path.join(path),
                source,
            })
    }

    pub fn check_file(&self, path: &Path, overwrite: bool) -> Result<(), ExtractError> {
        check_destination(&self.dir, path, overwrite).map_err(|source| {
            ExtractError::CreateFileFailed {
                path: self.path.join(path),
                source,
            }
        })
    }

    pub fn create_file(&self, path: &Path, overwrite: bool) -> Result<StagedFile, ExtractError> {
        self.check_file(path, overwrite)?;
        let create = || -> io::Result<StagedFile> {
            let parent_path = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let parent = self.dir.open_dir(parent_path)?;
            let destination = PathBuf::from(
                path.file_name()
                    .ok_or_else(|| io::Error::other("missing destination filename"))?,
            );
            check_destination(&parent, &destination, overwrite)?;
            for _ in 0..128 {
                let temporary = format!(".orbis-{:032x}.tmp", fastrand::u128(..));
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                match parent.open_with(&temporary, &options) {
                    Ok(file) => {
                        return Ok(StagedFile {
                            parent,
                            temporary,
                            destination,
                            display_path: self.path.join(path),
                            file: Some(file),
                            overwrite,
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::other("cannot allocate a temporary output file"))
        };
        create().map_err(|source| ExtractError::CreateFileFailed {
            path: self.path.join(path),
            source,
        })
    }
}

pub(super) fn valid_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    !name.is_empty()
        && !name.contains(['/', '\\', ':', '\0'])
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
}
