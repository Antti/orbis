//! Read-only output filesystem inspection; capability estimates never probe by writing.
use super::{Result, invalid};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct FilesystemPlan {
    pub filesystem: String,
    pub available_bytes: u64,
    /// Conservative reserve for staging plus a complete fallback copy, rounded
    /// to allocation units, with directory/manifest overhead.
    pub required_free_bytes: u64,
    pub maximum_file_bytes: Option<u64>,
    pub warnings: Vec<String>,
}

impl FilesystemPlan {
    pub(super) fn inspect(
        parent: &Path,
        sizes: impl Iterator<Item = u64>,
        entries: usize,
    ) -> Result<Self> {
        let filesystem = filesystem_name(parent).unwrap_or_else(|| "unknown".into());
        let stats = fs2::statvfs(parent)?;
        let unit = stats.allocation_granularity().max(4096);
        let maximum_file_bytes = if ["fat", "fat32", "msdos", "vfat"].contains(&filesystem.as_str())
        {
            Some(u64::from(u32::MAX))
        } else {
            None
        };
        let mut allocated = 0u64;
        for size in sizes {
            if maximum_file_bytes.is_some_and(|max| size > max) {
                return Err(invalid(
                    "output filesystem cannot hold files larger than 4 GiB minus one byte; use exFAT, APFS, NTFS, or ext4",
                ));
            }
            allocated = allocated
                .checked_add(
                    size.checked_add(unit - 1)
                        .ok_or_else(|| invalid("export size overflows"))?
                        / unit
                        * unit,
                )
                .ok_or_else(|| invalid("export size overflows"))?;
        }
        let overhead = (entries as u64)
            .checked_add(4)
            .and_then(|n| n.checked_mul(unit))
            .and_then(|n| n.checked_add(1024 * 1024))
            .ok_or_else(|| invalid("export size overflows"))?;
        let required_free_bytes = allocated
            .checked_add(overhead)
            .and_then(|n| n.checked_mul(2))
            .ok_or_else(|| invalid("export size overflows"))?;
        Ok(Self {
            filesystem,
            available_bytes: stats.available_space(), required_free_bytes, maximum_file_bytes,
            warnings: vec!["Space estimate reserves two copies for filesystems without exclusive rename. Actual filesystem quotas, metadata overhead, and concurrent writes can still cause failure.".into(),
                "Fallback publication is visible while copying; use the output only after export succeeds.".into()],
        })
    }
    pub(super) fn require_space(&self) -> Result<()> {
        if self.available_bytes < self.required_free_bytes {
            return Err(invalid(format!(
                "insufficient export space: reserve {} bytes, {} available",
                self.required_free_bytes, self.available_bytes
            )));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn filesystem_name(path: &Path) -> Option<String> {
    let stat = rustix::fs::statfs(path).ok()?;
    let bytes: Vec<_> = stat
        .f_fstypename
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    Some(String::from_utf8_lossy(&bytes).to_ascii_lowercase())
}
#[cfg(target_os = "linux")]
fn filesystem_name(path: &Path) -> Option<String> {
    let stat = rustix::fs::statfs(path).ok()?;
    Some(
        match stat.f_type as u64 {
            0x2011bab0 => "exfat",
            0x4d44 => "vfat",
            0xef53 => "ext4",
            0x9123683e => "btrfs",
            0x58465342 => "xfs",
            0x5346544e => "ntfs",
            _ => "unknown",
        }
        .into(),
    )
}
#[cfg(windows)]
fn filesystem_name(path: &Path) -> Option<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationByHandleW;
    let dir = cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority()).ok()?;
    let mut name = [0u16; 64];
    // All optional output pointers are null; the filesystem name buffer is live.
    if unsafe {
        GetVolumeInformationByHandleW(
            dir.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    } == 0
    {
        return None;
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..end]).to_ascii_lowercase())
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn filesystem_name(_: &Path) -> Option<String> {
    None
}
