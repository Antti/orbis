//! Publish staged output without replacing a concurrently created destination.
use cap_std::fs::{Dir, File, OpenOptions};
use std::{
    io,
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Prefer atomic publication. Filesystems such as macOS exFAT reject exclusive
/// rename and hard links, but still support exclusive file creation. On those
/// filesystems copy the completed staging file into a newly created destination.
/// This preserves no-overwrite semantics, but readers can see the final copy in
/// progress. The caller owns cleanup of the original staging file in either case.
pub(crate) fn publish_new_file(
    parent: &Dir,
    temporary: &str,
    destination: &Path,
    checkpoint: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    publish_file_with(parent, temporary, destination, checkpoint, publish_new)
}

fn publish_file_with(
    parent: &Dir,
    temporary: &str,
    destination: &Path,
    mut checkpoint: impl FnMut() -> io::Result<()>,
    rename: impl FnOnce(&Dir, &str, &Path) -> io::Result<()>,
) -> io::Result<()> {
    checkpoint()?;
    match rename(parent, temporary, destination) {
        Err(error) if unsupported_publication(&error) => {}
        result => return result,
    }
    let mut source = parent.open(temporary)?;
    let metadata = source.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "copy publication requires a regular file",
        ));
    }
    // O_EXCL/create_new refuses existing files, directories, and symlinks. Never
    // emulate NOREPLACE with an existence check followed by ordinary rename.
    let mut output = parent.open_with(
        destination,
        OpenOptions::new().read(true).write(true).create_new(true),
    )?;
    let result = (|| {
        let mut buffer = vec![0; 1024 * 1024];
        let mut copied = 0;
        loop {
            checkpoint()?;
            let n = source.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
            copied += n as u64;
        }
        if copied != metadata.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "staging file changed during publication",
            ));
        }
        output.sync_all()
    })();
    if result.is_err() {
        // exFAT changes an empty file's synthetic inode when its first cluster
        // is allocated. Read the identity from the live handle after copying,
        // rather than saving the empty file's identity before the first write.
        let identity = file_identity(&output);
        drop(output);
        if identity.is_ok_and(|identity| still_owned(parent, destination, identity)) {
            let _ = parent.remove_file(destination);
        }
    }
    result
}

pub(crate) fn unsupported_publication(error: &io::Error) -> bool {
    // Darwin has distinct ENOTSUP (45) and EOPNOTSUPP (102). Rust currently
    // classifies the former as Uncategorized, though exFAT returns it here.
    #[cfg(target_os = "macos")]
    if error.raw_os_error() == Some(rustix::io::Errno::NOTSUP.raw_os_error()) {
        return true;
    }
    // Linux filesystems (including older FUSE drivers) may reject the
    // RENAME_NOREPLACE flag with EINVAL. All callers use validated relative
    // paths and this one flag; exclusive-create fallback still refuses conflicts.
    #[cfg(target_os = "linux")]
    if error.raw_os_error() == Some(rustix::io::Errno::INVAL.raw_os_error()) {
        return true;
    }
    error.kind() == io::ErrorKind::Unsupported
}

/// Publish a completed directory on filesystems without exclusive rename.
/// The destination is reserved with mkdir, then populated using exclusive file
/// creation. It is visible while copying and requires space for both trees.
/// Copy only planned paths, parents before children: the filesystem may have
/// generated metadata sidecars in staging which must not be copied as game data.
pub(crate) fn copy_new_directory<'a>(
    parent: &Dir,
    temporary: &str,
    destination: &Path,
    entries: impl IntoIterator<Item = (&'a Path, bool)>,
    mut checkpoint: impl FnMut(u64) -> io::Result<()>,
) -> io::Result<()> {
    checkpoint(0)?;
    let source = parent.open_dir(temporary)?;
    parent.create_dir(destination)?; // Fails even for an existing empty directory.
    let dir = parent.open_dir(destination)?;
    let identity = directory_identity(&dir)?;
    let mut copy = DirectoryCopy {
        parent,
        destination,
        dir: Some(dir),
        identity,
        created: Vec::new(),
        complete: false,
    };
    let mut buffer = vec![0; 1024 * 1024];
    for (path, directory) in entries {
        checkpoint(0)?;
        let meta = source.symlink_metadata(path)?;
        let output_dir = copy.dir.as_ref().unwrap();
        if directory && meta.is_dir() {
            output_dir.create_dir(path)?;
            let identity = directory_identity(&output_dir.open_dir(path)?)?;
            copy.created.push((path.to_owned(), true, identity));
        } else if !directory && meta.is_file() {
            let mut input = source.open(path)?;
            if !input.metadata()?.is_file() {
                return Err(io::Error::other("staging file changed during publication"));
            }
            let mut output = output_dir.open_with(
                path,
                OpenOptions::new().read(true).write(true).create_new(true),
            )?;
            let result = (|| {
                let mut bytes = 0;
                loop {
                    checkpoint(0)?;
                    let n = input.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    output.write_all(&buffer[..n])?;
                    bytes += n as u64;
                    checkpoint(n as u64)?;
                }
                if bytes != meta.len() {
                    return Err(io::Error::other("staging file changed during publication"));
                }
                output.sync_all()
            })();
            // exFAT assigns a new identity on the first write. Record the
            // live handle after copying, including when the copy failed.
            copy.created
                .push((path.to_owned(), false, file_identity(&output)?));
            result?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "staging entry type changed during publication",
            ));
        }
    }
    // Children must reach disk before their parent. Windows does not support
    // the directory syncing used here, matching the export's normal path.
    #[cfg(unix)]
    {
        let dir = copy.dir.as_ref().unwrap();
        for (path, directory, _) in copy.created.iter().rev() {
            checkpoint(0)?;
            if *directory {
                dir.open_dir(path)?.open(".")?.sync_all()?;
            }
        }
        dir.open(".")?.sync_all()?;
    }
    checkpoint(0)?;
    if !directory_still_owned(parent, destination, identity) {
        return Err(io::Error::other(
            "export destination changed during publication",
        ));
    }
    copy.complete = true;
    Ok(())
}

struct DirectoryCopy<'a> {
    parent: &'a Dir,
    destination: &'a Path,
    dir: Option<Dir>,
    identity: (u64, u64),
    // Creation order guarantees child directories are removed before parents.
    created: Vec<(PathBuf, bool, (u64, u64))>,
    complete: bool,
}

impl Drop for DirectoryCopy<'_> {
    fn drop(&mut self) {
        if self.complete || !directory_still_owned(self.parent, self.destination, self.identity) {
            return;
        }
        let dir = self.dir.as_ref().unwrap();
        for (path, directory, identity) in self.created.iter().rev() {
            if *directory {
                if directory_still_owned(dir, path, *identity) {
                    // Never recursively remove entries created by someone else.
                    let _ = dir.remove_dir(path);
                }
            } else if still_owned(dir, path, *identity) {
                let _ = dir.remove_file(path);
            }
        }
        // Windows requires closing our directory handle before removal.
        self.dir.take();
        if directory_still_owned(self.parent, self.destination, self.identity) {
            let _ = self.parent.remove_dir(self.destination);
        }
    }
}

fn directory_identity(dir: &Dir) -> io::Result<(u64, u64)> {
    file_identity(&File::from_std(dir.try_clone()?.into_std_file()))
}

fn directory_still_owned(parent: &Dir, path: &Path, identity: (u64, u64)) -> bool {
    parent
        .symlink_metadata(path)
        .is_ok_and(|meta| meta.is_dir())
        && parent
            .open_dir(path)
            .and_then(|dir| directory_identity(&dir))
            .is_ok_and(|current| current == identity)
}

fn still_owned(parent: &Dir, path: &Path, identity: (u64, u64)) -> bool {
    parent
        .symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file())
        && parent
            .open(path)
            .and_then(|file| file_identity(&file))
            .is_ok_and(|current| current == identity)
}

#[cfg(unix)]
fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    use cap_std::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    use std::{mem::MaybeUninit, os::windows::io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // The file handle remains live and the API initializes info on success.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_file: &File) -> io::Result<(u64, u64)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "file identity unavailable",
    ))
}

pub(crate) fn publish_new(parent: &Dir, temporary: &str, destination: &Path) -> io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::ambient_authority;

    fn unsupported(_: &Dir, _: &str, _: &Path) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        return Err(io::Error::from_raw_os_error(
            rustix::io::Errno::NOTSUP.raw_os_error(),
        ));
        #[cfg(not(target_os = "macos"))]
        Err(io::ErrorKind::Unsupported.into())
    }

    fn setup() -> (tempfile::TempDir, Dir) {
        let temp = match std::env::var_os("ORBIS_TEST_EXTRACT_ROOT") {
            Some(root) => tempfile::tempdir_in(root).unwrap(),
            None => tempfile::tempdir().unwrap(),
        };
        let dir = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        dir.write("staged", b"COMPLETE").unwrap();
        (temp, dir)
    }

    fn directory_setup() -> (tempfile::TempDir, Dir) {
        let temp = match std::env::var_os("ORBIS_TEST_EXPORT_ROOT") {
            Some(root) => tempfile::tempdir_in(root).unwrap(),
            None => tempfile::tempdir().unwrap(),
        };
        let dir = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        dir.create_dir_all("tree/nested/empty").unwrap();
        dir.write("tree/nested/file", vec![b'A'; 2 * 1024 * 1024])
            .unwrap();
        (temp, dir)
    }

    fn copy_test_directory(
        dir: &Dir,
        checkpoint: impl FnMut(u64) -> io::Result<()>,
    ) -> io::Result<()> {
        copy_new_directory(
            dir,
            "tree",
            Path::new("final"),
            [
                (Path::new("nested"), true),
                (Path::new("nested/empty"), true),
                (Path::new("nested/file"), false),
            ],
            checkpoint,
        )
    }

    #[test]
    fn directory_copy_preserves_contents_and_empty_directories() {
        let (_temp, dir) = directory_setup();
        let mut bytes = 0;
        copy_test_directory(&dir, |n| {
            bytes += n;
            Ok(())
        })
        .unwrap();
        assert_eq!(bytes, 2 * 1024 * 1024);
        assert!(dir.metadata("final/nested/empty").unwrap().is_dir());
        assert_eq!(
            dir.read("final/nested/file").unwrap(),
            dir.read("tree/nested/file").unwrap()
        );
        dir.write("final/nested/file", b"EDITED").unwrap();
        assert_eq!(dir.metadata("tree/nested/file").unwrap().len(), bytes);
    }

    #[test]
    fn directory_copy_refuses_existing_destinations() {
        for directory in [false, true] {
            let (_temp, dir) = directory_setup();
            if directory {
                dir.create_dir("final").unwrap();
            } else {
                dir.write("final", b"KEEP").unwrap();
            }
            assert!(copy_test_directory(&dir, |_| Ok(())).is_err());
            if directory {
                assert_eq!(dir.open_dir("final").unwrap().entries().unwrap().count(), 0);
            } else {
                assert_eq!(dir.read("final").unwrap(), b"KEEP");
            }
        }
    }

    #[test]
    fn directory_copy_cleans_up_after_cancellation_and_write_failure() {
        for kind in [io::ErrorKind::Interrupted, io::ErrorKind::StorageFull] {
            let (_temp, dir) = directory_setup();
            let error =
                copy_test_directory(
                    &dir,
                    |bytes| {
                        if bytes > 0 { Err(kind.into()) } else { Ok(()) }
                    },
                )
                .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert!(!dir.try_exists("final").unwrap());
            assert_eq!(
                dir.metadata("tree/nested/file").unwrap().len(),
                2 * 1024 * 1024
            );
        }
    }

    #[test]
    fn directory_copy_cleanup_preserves_concurrently_added_files() {
        let (_temp, dir) = directory_setup();
        let error = copy_test_directory(&dir, |bytes| {
            if bytes > 0 {
                dir.write("final/nested/keep", b"KEEP")?;
                Err(io::ErrorKind::Interrupted.into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(dir.read("final/nested/keep").unwrap(), b"KEEP");
        assert!(!dir.try_exists("final/nested/file").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn directory_copy_cleanup_preserves_replacement_files_and_roots() {
        for replace_root in [false, true] {
            let (_temp, dir) = directory_setup();
            dir.write("replacement", b"KEEP").unwrap();
            let error = copy_test_directory(&dir, |bytes| {
                if bytes > 0 {
                    if replace_root {
                        dir.rename("final", &dir, "moved")?;
                        dir.create_dir("final")?;
                        dir.write("final/keep", b"KEEP")?;
                    } else {
                        dir.rename("replacement", &dir, "final/nested/file")?;
                    }
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Interrupted);
            assert_eq!(
                dir.read(if replace_root {
                    "final/keep"
                } else {
                    "final/nested/file"
                })
                .unwrap(),
                b"KEEP"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn directory_copy_refuses_symlinks_in_staging_and_at_destination() {
        use std::os::unix::fs::symlink;
        let (temp, dir) = directory_setup();
        symlink("missing", temp.path().join("final")).unwrap();
        assert!(copy_test_directory(&dir, |_| Ok(())).is_err());
        assert_eq!(dir.read_link("final").unwrap(), Path::new("missing"));
        dir.remove_file("final").unwrap();
        dir.remove_file("tree/nested/file").unwrap();
        symlink("empty", temp.path().join("tree/nested/file")).unwrap();
        assert!(copy_test_directory(&dir, |_| Ok(())).is_err());
        assert!(!dir.try_exists("final").unwrap());
    }

    #[test]
    fn unsupported_rename_copies_complete_independent_file() {
        let (_temp, dir) = setup();
        publish_file_with(&dir, "staged", Path::new("final"), || Ok(()), unsupported).unwrap();
        assert_eq!(dir.read("final").unwrap(), b"COMPLETE");
        dir.write("final", b"EDITED").unwrap();
        assert_eq!(dir.read("staged").unwrap(), b"COMPLETE");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unsupported_rename_flags_use_exclusive_copy() {
        let (_temp, dir) = setup();
        publish_file_with(
            &dir,
            "staged",
            Path::new("final"),
            || Ok(()),
            |_, _, _| {
                Err(io::Error::from_raw_os_error(
                    rustix::io::Errno::INVAL.raw_os_error(),
                ))
            },
        )
        .unwrap();
        assert_eq!(dir.read("final").unwrap(), b"COMPLETE");
        assert!(
            publish_file_with(
                &dir,
                "staged",
                Path::new("final"),
                || Ok(()),
                |_, _, _| {
                    Err(io::Error::from_raw_os_error(
                        rustix::io::Errno::INVAL.raw_os_error(),
                    ))
                }
            )
            .is_err()
        );
        assert_eq!(dir.read("final").unwrap(), b"COMPLETE");
    }

    #[test]
    fn other_rename_errors_do_not_trigger_a_copy() {
        for kind in [
            io::ErrorKind::AlreadyExists,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::Other,
        ] {
            let (_temp, dir) = setup();
            let error = publish_file_with(
                &dir,
                "staged",
                Path::new("final"),
                || Ok(()),
                |_, _, _| Err(kind.into()),
            )
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert!(!dir.try_exists("final").unwrap());
            assert_eq!(dir.read("staged").unwrap(), b"COMPLETE");
        }
    }

    #[test]
    fn fallback_preserves_files_and_directories_created_before_exclusive_open() {
        for directory in [false, true] {
            let (_temp, dir) = setup();
            let error = publish_file_with(
                &dir,
                "staged",
                Path::new("final"),
                || Ok(()),
                |parent, _, path| {
                    if directory {
                        parent.create_dir(path)?;
                    } else {
                        parent.write(path, b"KEEP")?;
                    }
                    Err(io::ErrorKind::Unsupported.into())
                },
            )
            .unwrap_err();
            // Windows may report access denied for an existing destination.
            // Both errors must leave the conflicting entry untouched.
            assert!(matches!(
                error.kind(),
                io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied
            ));
            if directory {
                assert!(dir.metadata("final").unwrap().is_dir());
            } else {
                assert_eq!(dir.read("final").unwrap(), b"KEEP");
            }
        }
    }

    #[test]
    fn failed_or_cancelled_copy_removes_only_its_partial_destination() {
        for kind in [io::ErrorKind::Interrupted, io::ErrorKind::StorageFull] {
            let (_temp, dir) = setup();
            dir.write("staged", vec![b'A'; 2 * 1024 * 1024]).unwrap();
            let mut calls = 0;
            let error = publish_file_with(
                &dir,
                "staged",
                Path::new("final"),
                || {
                    calls += 1;
                    if calls == 3 {
                        assert_eq!(dir.metadata("final").unwrap().len(), 1024 * 1024);
                        Err(kind.into())
                    } else {
                        Ok(())
                    }
                },
                unsupported,
            )
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert!(!dir.try_exists("final").unwrap());
            assert_eq!(dir.metadata("staged").unwrap().len(), 2 * 1024 * 1024);
        }
    }

    #[cfg(unix)]
    #[test]
    fn fallback_refuses_symlinks_and_cleanup_preserves_a_replacement() {
        use std::os::unix::fs::symlink;
        let (temp, dir) = setup();
        dir.write("other", b"KEEP").unwrap();
        symlink("other", temp.path().join("final")).unwrap();
        assert!(
            publish_file_with(&dir, "staged", Path::new("final"), || Ok(()), unsupported).is_err()
        );
        assert_eq!(dir.read("other").unwrap(), b"KEEP");
        dir.remove_file("final").unwrap();
        let mut calls = 0;
        let error = publish_file_with(
            &dir,
            "staged",
            Path::new("final"),
            || {
                calls += 1;
                if calls == 3 {
                    dir.rename("other", &dir, "final")?;
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(())
                }
            },
            unsupported,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(dir.read("final").unwrap(), b"KEEP");
    }

    #[test]
    fn fallback_never_copies_a_directory_as_a_file() {
        let (_temp, dir) = setup();
        dir.create_dir("directory").unwrap();
        assert!(
            publish_file_with(
                &dir,
                "directory",
                Path::new("final"),
                || Ok(()),
                unsupported
            )
            .is_err()
        );
        assert!(!dir.try_exists("final").unwrap());
    }
}
