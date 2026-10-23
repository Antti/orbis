//! Bounded manifest reads and durable replacement through a temporary file.
use super::{InventoryError, Result, invalid};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{Read, Write};

pub(crate) const MAX_JSON: u64 = 64 * 1024 * 1024;

pub(crate) fn read_json<T: DeserializeOwned>(dir: &Dir, name: &str) -> Result<T> {
    if !dir.symlink_metadata(name)?.is_file() {
        return Err(invalid("expected a regular file, not a symlink"));
    }
    let mut data = Vec::new();
    dir.open(name)?.take(MAX_JSON + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_JSON {
        return Err(InventoryError::ManifestTooLarge);
    }
    Ok(serde_json::from_slice(&data)?)
}
pub(crate) fn write_json(dir: &Dir, name: &str, value: &impl Serialize) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    // Never replace a manifest with one that our own reader refuses.
    if data.len() as u64 > MAX_JSON {
        return Err(InventoryError::ManifestTooLarge);
    }
    let tmp = format!(".write-{:032x}.tmp", fastrand::u128(..));
    let result = (|| {
        let mut file = dir.open_with(&tmp, OpenOptions::new().write(true).create_new(true))?;
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        dir.rename(&tmp, dir, name)?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = dir.remove_file(tmp);
    }
    result
}
pub(crate) fn sync_dir(dir: &Dir) -> Result<()> {
    #[cfg(unix)]
    // On Linux, Dir may hold an O_PATH descriptor, which fsync rejects with
    // EBADF. Reopen "." for reading while retaining the directory capability.
    dir.open(".")?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}
