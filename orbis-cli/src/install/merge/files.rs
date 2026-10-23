//! Filesystem operations used by the merge journal. Never overwrite a destination.
use super::*;
use std::{collections::BTreeMap, ffi::OsStr, io};

/// Inspect names and entry types without hashing the whole game.
pub(super) fn namespace(
    root: &Dir,
    progress: &impl ExtractProgress,
) -> Result<BTreeMap<String, bool>> {
    let mut found = BTreeMap::new();
    let mut folded = BTreeMap::new();
    let mut pending = vec![String::new()];
    while let Some(prefix) = pending.pop() {
        cancelled(progress)?;
        let directory = checked_dir(
            root,
            Path::new(if prefix.is_empty() { "." } else { &prefix }),
        )?;
        for entry in directory.entries()? {
            cancelled(progress)?;
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("non-UTF8 game filename"))?;
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            storage::validate_path(&path)?;
            let meta = directory.symlink_metadata(entry.file_name())?;
            if !meta.is_dir() && !meta.is_file() {
                return Err(invalid(format!(
                    "merge requires regular files and directories: {path}"
                )));
            }
            insert_name(&mut folded, &path, meta.is_dir())?;
            found.insert(path.clone(), meta.is_dir());
            if meta.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(found)
}

pub(super) fn insert_name(
    names: &mut BTreeMap<String, (String, bool)>,
    path: &str,
    directory: bool,
) -> Result<()> {
    storage::validate_path(path)?;
    let key = path.to_ascii_lowercase();
    if let Some((old, kind)) = names.get(&key) {
        if old != path || *kind != directory {
            return Err(invalid(format!(
                "case or file/directory conflict: {old} / {path}"
            )));
        }
    } else {
        names.insert(key, (path.into(), directory));
    }
    if let Some(parent) = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        insert_name(names, parent.to_str().unwrap(), true)?;
    }
    Ok(())
}

pub(super) fn checked_dir(root: &Dir, path: &Path) -> Result<Dir> {
    let mut dir = root.try_clone()?;
    for part in path.components() {
        if part == std::path::Component::CurDir {
            continue;
        }
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(invalid("invalid merge directory path"));
        }
        storage::require_directory(&dir, part.as_os_str())?;
        dir = dir.open_dir(part.as_os_str())?;
    }
    Ok(dir)
}

pub(super) fn parents(root: &Dir, path: &str) -> Result<()> {
    let mut dir = root.try_clone()?;
    if let Some(parent) = Path::new(path).parent() {
        for part in parent.components() {
            if !exists(&dir, part.as_os_str())? {
                dir.create_dir(part.as_os_str())?;
                sync_dir(&dir)?;
            }
            storage::require_directory(&dir, part.as_os_str())?;
            dir = dir.open_dir(part.as_os_str())?;
        }
    }
    Ok(())
}

pub(super) fn move_file(from: &Dir, to: &Dir, path: &str) -> Result<()> {
    storage::validate_path(path)?;
    let path = Path::new(path);
    let source = checked_dir(from, path.parent().unwrap())?;
    let destination = checked_dir(to, path.parent().unwrap())?;
    let name = path.file_name().unwrap();
    if !source.symlink_metadata(name)?.is_file() {
        return Err(invalid("merge source is no longer a regular file"));
    }
    exclusive_move(&source, name, &destination, name)?;
    sync_dir(&source)?;
    sync_dir(&destination)
}

pub(super) fn exclusive_move(
    from: &Dir,
    source: &OsStr,
    to: &Dir,
    destination: &OsStr,
) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        rustix::fs::renameat_with(
            from,
            source,
            to,
            destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        // Hold both directory handles. cap-std denies directory deletion on
        // Windows; only a validated leaf name is joined to each handle's path.
        let a = from.try_clone()?.into_std_file();
        let b = to.try_clone()?.into_std_file();
        let source: Vec<_> = winx::file::get_file_path(&a)?
            .join(source)
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        let destination: Vec<_> = winx::file::get_file_path(&b)?
            .join(destination)
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (from, source, to, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "merge requires exclusive rename",
        ))
    }
}

pub(super) fn probe(root: &Dir) -> Result<()> {
    root.write("rename-probe", b"")?;
    exclusive_move(root, OsStr::new("rename-probe"), root, OsStr::new("rename-probe-done"))
        .map_err(|e| invalid(format!("this filesystem cannot safely merge files ({e}); use separate updates or Export instead")))?;
    root.remove_file("rename-probe-done")?;
    Ok(())
}
