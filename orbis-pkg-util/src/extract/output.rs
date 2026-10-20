use super::ExtractError;
use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};
use std::path::{Component, Path, PathBuf};

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

    pub fn child(&self, name: &str) -> Result<Self, ExtractError> {
        self.create_dir(Path::new(name))?;
        let dir =
            self.dir
                .open_dir(name)
                .map_err(|source| ExtractError::CreateDirectoryFailed {
                    path: self.path.join(name),
                    source,
                })?;
        Ok(Self {
            dir,
            path: self.path.join(name),
        })
    }

    pub fn create_file(&self, path: &Path, overwrite: bool) -> Result<File, ExtractError> {
        let mut options = OpenOptions::new();
        options.write(true);
        if overwrite {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        self.dir
            .open_with(path, &options)
            .map_err(|source| ExtractError::CreateFileFailed {
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
