use super::ExtractError;
use crate::{ExtractProgress, fs_output::publish_new_file};
use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};
use std::path::{Path, PathBuf};
use std::{
    io,
    io::{Read, Write},
};

/// Policy resolved once from the public extraction builder options.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ExistingFilePolicy {
    Refuse,
    Replace,
    Verify,
}

impl ExistingFilePolicy {
    pub fn from_options(overwrite: bool, resume: bool) -> Self {
        if resume {
            Self::Verify
        } else if overwrite {
            Self::Replace
        } else {
            Self::Refuse
        }
    }
}

/// Existing files are compared with the decoded source stream; no bytes are written.
pub(super) enum FileOutput {
    Staged(StagedFile),
    Existing { file: File, path: PathBuf },
}
impl Write for FileOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Staged(file) => file.write(bytes),
            Self::Existing { file, .. } => {
                let mut buffer = [0u8; 64 * 1024];
                for chunk in bytes.chunks(buffer.len()) {
                    file.read_exact(&mut buffer[..chunk.len()]).map_err(|_| io::Error::other("resume file is truncated; preserve or remove the conflicting file before retrying"))?;
                    if chunk != &buffer[..chunk.len()] {
                        return Err(io::Error::other(
                            "resume file differs from the package; preserve or remove the conflicting file before retrying",
                        ));
                    }
                }
                Ok(bytes.len())
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Staged(file) => file.flush(),
            Self::Existing { .. } => Ok(()),
        }
    }
}
impl FileOutput {
    pub fn commit(self, progress: &impl ExtractProgress) -> Result<(), ExtractError> {
        match self {
            Self::Staged(file) => file.commit(progress),
            Self::Existing { mut file, path } => {
                let mut extra = [0u8; 1];
                let result = file.read(&mut extra).and_then(|n| {
                    if n == 0 {
                        Ok(())
                    } else {
                        Err(io::Error::other(
                            "resume file is longer than the package file",
                        ))
                    }
                });
                result.map_err(|source| ExtractError::WriteFailed { path, source })
            }
        }
    }
}

/// Stages a file beside its destination, using the same confined directory
/// handle for creation, publication, and cleanup.
pub(super) struct StagedFile {
    parent: Dir,
    temporary: String,
    destination: PathBuf,
    display_path: PathBuf,
    file: Option<File>,
    policy: ExistingFilePolicy,
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
    pub fn commit(mut self, progress: &impl ExtractProgress) -> Result<(), ExtractError> {
        self.file
            .take()
            .unwrap()
            .sync_all()
            .map_err(|source| ExtractError::WriteFailed {
                path: self.display_path.clone(),
                source,
            })?;
        let result = if self.policy == ExistingFilePolicy::Replace {
            // rename replaces a symlink itself, never its target. Refuse known
            // symlinks anyway so --force retains the extractor's existing policy.
            check_destination(&self.parent, &self.destination, ExistingFilePolicy::Replace)
                .and_then(|()| {
                    self.parent
                        .rename(&self.temporary, &self.parent, &self.destination)
                })
        } else {
            publish_new_file(&self.parent, &self.temporary, &self.destination, || {
                if progress.is_cancelled() {
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(())
                }
            })
        };
        result.map_err(|source| {
            if source.kind() == io::ErrorKind::Interrupted && progress.is_cancelled() {
                ExtractError::Cancelled
            } else {
                ExtractError::CreateFileFailed {
                    path: self.display_path.clone(),
                    source,
                }
            }
        })
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        self.file.take();
        let _ = self.parent.remove_file(&self.temporary);
    }
}

fn check_destination(dir: &Dir, path: &Path, policy: ExistingFilePolicy) -> io::Result<()> {
    match dir.symlink_metadata(path) {
        Ok(metadata) if policy != ExistingFilePolicy::Refuse && metadata.is_file() => Ok(()),
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

    pub fn check_file(&self, path: &Path, policy: ExistingFilePolicy) -> Result<(), ExtractError> {
        check_destination(&self.dir, path, policy).map_err(|source| {
            ExtractError::CreateFileFailed {
                path: self.path.join(path),
                source,
            }
        })
    }

    pub fn create_file(
        &self,
        path: &Path,
        policy: ExistingFilePolicy,
    ) -> Result<FileOutput, ExtractError> {
        if policy == ExistingFilePolicy::Verify {
            let open = || -> io::Result<Option<File>> {
                let mut prefix = PathBuf::new();
                for part in path.components() {
                    prefix.push(part);
                    match self.dir.symlink_metadata(&prefix) {
                        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                        Err(e) => return Err(e),
                        Ok(meta)
                            if (prefix != path && meta.is_dir())
                                || (prefix == path && meta.is_file()) => {}
                        Ok(_) => {
                            return Err(io::Error::other(
                                "resume refuses symlinks and special files",
                            ));
                        }
                    }
                }
                Ok(Some(self.dir.open(path)?))
            };
            if let Some(file) = open().map_err(|source| ExtractError::CreateFileFailed {
                path: self.path.join(path),
                source,
            })? {
                return Ok(FileOutput::Existing {
                    file,
                    path: self.path.join(path),
                });
            }
        }
        // A missing resume file must still be published without replacing a file
        // that appeared after verification.
        let publication = if policy == ExistingFilePolicy::Replace {
            ExistingFilePolicy::Replace
        } else {
            ExistingFilePolicy::Refuse
        };
        self.create_staged_file(path, publication)
            .map(FileOutput::Staged)
    }

    fn create_staged_file(
        &self,
        path: &Path,
        policy: ExistingFilePolicy,
    ) -> Result<StagedFile, ExtractError> {
        self.check_file(path, policy)?;
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
            check_destination(&parent, &destination, policy)?;
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
                            policy,
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
    orbis_pkg::path::ComponentPolicy::Extraction.accepts(name)
}
