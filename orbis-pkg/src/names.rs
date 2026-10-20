use crate::{
    FindEntryError, Pkg,
    entry::{EntryId, PkgEntry},
};
use std::{fmt, path::PathBuf};

#[derive(Debug)]
pub struct EntryNameError(pub u32);
impl fmt::Display for EntryNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid name table or filename for entry 0x{:08X}",
            self.0
        )
    }
}
impl std::error::Error for EntryNameError {}

impl<R: AsRef<[u8]>> Pkg<R> {
    /// Resolves an extraction path relative to `sce_sys`. Known IDs retain their
    /// canonical paths. Unknown IDs use the name table, or `entry_XXXXXXXX.bin`
    /// when unnamed. Malformed/unsafe names are errors, never host paths.
    pub fn entry_path(&self, entry: &PkgEntry) -> Result<PathBuf, EntryNameError> {
        if let Some(path) = entry.to_path("") {
            return Ok(path);
        }
        let bad = || EntryNameError(entry.id());
        let fallback = || PathBuf::from(format!("entry_{:08X}.bin", entry.id()));
        let (names, _) = match self.find_entry(EntryId::EntryNames) {
            Ok(found) => found,
            Err(FindEntryError::NotFound) => return Ok(fallback()),
            Err(_) => return Err(bad()),
        };
        // Name tables are normally plaintext. Keep header/table inspection
        // independent of keys and avoid allocating an archive-sized buffer.
        if names.is_encrypted() {
            return Err(bad());
        }
        let data = self
            .raw
            .as_ref()
            .get(names.data_offset()..)
            .and_then(|b| b.get(..names.data_size()))
            .ok_or_else(bad)?;
        let offset = entry.filename_offset();
        if data.is_empty() && offset == 0 {
            return Ok(fallback());
        }
        if offset > 0 && data.get(offset - 1) != Some(&0) {
            return Err(bad());
        }
        let tail = data.get(offset..).ok_or_else(bad)?;
        let end = tail
            .iter()
            .take(2049)
            .position(|b| *b == 0)
            .ok_or_else(bad)?;
        if end == 0 {
            return Ok(fallback());
        }
        let name = std::str::from_utf8(&tail[..end]).map_err(|_| bad())?;
        if !name.split('/').all(safe_component) {
            return Err(bad());
        }
        Ok(name.split('/').collect())
    }
}

// Enforce the same portable rules on every host, including Windows aliases,
// device names and alternate data streams.
fn safe_component(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name
            .chars()
            .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
        && !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        && !(stem.starts_with("COM") || stem.starts_with("LPT"))
            .then(|| &stem[3..])
            .is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
}
