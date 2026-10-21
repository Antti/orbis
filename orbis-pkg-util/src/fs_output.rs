//! Publish staged output without replacing a concurrently created destination.
use cap_std::fs::Dir;
use std::{io, path::Path};

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
