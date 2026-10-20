//! Stored SHA-256 checks over the package's on-disk bytes.
//! These establish consistency, not publisher authenticity.
//! Format reference: <https://github.com/maxton/LibOrbisPkg/blob/master/LibOrbisPkg/PKG/PkgBuilder.cs>
use crate::{
    Pkg,
    entry::{EntryId, PkgEntry},
};
pub use orbis_integrity::{Check, IntegrityReport, Reason, Status};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, io};

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

impl<R: AsRef<[u8]>> Pkg<R> {
    /// Checks the header, body, SC entry chains, digest table and entry payloads.
    /// With `include_pfs`, also checks the stored full/signed-region PFS digests.
    /// Hashes ciphertext as stored, without needing decryption keys.
    /// All-zero/missing digests are counted, never accepted as verified.
    pub fn verify_integrity(
        &self,
        include_pfs: bool,
        cancelled: impl Fn() -> bool,
    ) -> io::Result<IntegrityReport> {
        self.verify_integrity_with(include_pfs, cancelled, |_| Ok(()))
    }

    /// Streams each check, including the first mismatch, before returning.
    /// Malformed structures and I/O errors remain errors, not missing checks.
    pub fn verify_integrity_with(
        &self,
        include_pfs: bool,
        cancelled: impl Fn() -> bool,
        mut observer: impl FnMut(Check<'_>) -> io::Result<()>,
    ) -> io::Result<IntegrityReport> {
        if cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let raw = self.raw.as_ref();
        let header = self.header.raw_header();
        // Validate before collecting an ExactSizeIterator: a corrupt count
        // must not become an unbounded allocation request.
        self.stored_range(
            self.header.table_offset() as u64,
            self.entry_count() as u64 * PkgEntry::RAW_SIZE as u64,
        )?;
        let entries = self
            .entries()
            .map(|e| e.map(|(_, e)| e).map_err(|e| invalid(e.to_string())))
            .collect::<io::Result<Vec<_>>>()?;
        let mut ids = HashSet::new();
        for entry in &entries {
            if !ids.insert(entry.id()) {
                return Err(invalid("duplicate PKG entry ID"));
            }
            self.stored_range(entry.data_offset() as u64, entry.data_size() as u64)?;
        }
        let mut report = IntegrityReport::default();
        let mut check = |name: &str, expected: &[u8], parts: &[&[u8]]| -> io::Result<()> {
            if cancelled() {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            if expected.iter().all(|b| *b == 0) {
                return report.record(
                    name,
                    Status::Unavailable,
                    Some(Reason::MissingDigest),
                    1,
                    &mut observer,
                );
            }
            let mut hash = Sha256::new();
            for part in parts {
                for chunk in part.chunks(1024 * 1024) {
                    if cancelled() {
                        return Err(io::Error::from(io::ErrorKind::Interrupted));
                    }
                    hash.update(chunk);
                }
            }
            if hash.finalize().as_slice() != expected {
                report.record(
                    name,
                    Status::Failed,
                    Some(Reason::DigestMismatch),
                    1,
                    &mut observer,
                )?;
                return Err(invalid(format!("SHA-256 mismatch: {name}")));
            }
            report.record(name, Status::Passed, None, 1, &mut observer)
        };
        check("PKG header", &header.pkg_digest, &[&raw[..0xfe0]])?;
        check(
            "PKG body",
            &header.digest_table.digest_body_digest,
            &[self.stored_range(header.pkg_body_offset.get(), header.pkg_body_size.get())?],
        )?;
        let find = |id| entries.iter().find(|e| e.entry_id() == id);
        let bytes = |entry: &PkgEntry| {
            self.stored_range(entry.data_offset() as u64, entry.data_size() as u64)
        };
        if let Some(digests) = find(EntryId::Digests) {
            let table = bytes(digests)?;
            if table.len() < entries.len() * 32 {
                return Err(invalid("truncated PKG digest table"));
            }
            check(
                "digest table",
                &header.digest_table.digest_table_digest,
                &[table],
            )?;
            for (i, entry) in entries.iter().enumerate() {
                if entry.entry_id() != EntryId::Digests {
                    check(
                        &format!("entry 0x{:08X}", entry.id()),
                        &table[i * 32..i * 32 + 32],
                        &[bytes(entry)?],
                    )?;
                }
            }
        } else {
            // One missing table hash plus the unavailable per-entry checks.
            check("digest table absent", &[0; 32], &[])?;
            for entry in &entries {
                check(&format!("entry 0x{:08X}", entry.id()), &[0; 32], &[])?;
            }
        }
        for (second, expected) in [
            (false, &header.digest_table.digest_entries1),
            (true, &header.digest_table.digest_entries2),
        ] {
            let mut parts = Vec::new();
            let chain = [
                EntryId::EntryKeys,
                EntryId::PfsImageKey,
                EntryId::GeneralDigests,
                EntryId::Metas,
                EntryId::Digests,
            ];
            for id in &chain[..if second { 4 } else { 5 }] {
                let Some(entry) = find(*id) else {
                    if expected.iter().any(|b| *b != 0) {
                        return Err(invalid("SC digest references a missing entry"));
                    }
                    continue;
                };
                let data = bytes(entry)?;
                parts.push(if second && *id == EntryId::Metas {
                    data.get(..usize::from(header.pkg_sc_entry_count.get()) * 32)
                        .ok_or_else(|| invalid("SC entry count exceeds metadata table"))?
                } else {
                    data
                });
            }
            check(
                if second {
                    "SC entries 2"
                } else {
                    "SC entries 1"
                },
                expected,
                &parts,
            )?;
        }
        if include_pfs && header.pfs_image_size.get() != 0 {
            let pfs =
                self.stored_range(header.pfs_image_offset.get(), header.pfs_image_size.get())?;
            let signed = pfs
                .get(..header.pfs_signed_size.get() as usize)
                .ok_or_else(|| invalid("signed PFS region exceeds image"))?;
            check("PFS image", &header.pfs_image_digest, &[pfs])?;
            check("signed PFS region", &header.pfs_signed_digest, &[signed])?;
        }
        Ok(report)
    }

    fn stored_range(&self, offset: u64, size: u64) -> io::Result<&[u8]> {
        let end = offset
            .checked_add(size)
            .ok_or_else(|| invalid("PKG integrity range overflow"))?;
        let (offset, end) = (usize::try_from(offset), usize::try_from(end));
        match (offset, end) {
            (Ok(start), Ok(end)) => self
                .raw
                .as_ref()
                .get(start..end)
                .ok_or_else(|| invalid("PKG integrity range outside file")),
            _ => Err(invalid("PKG integrity range exceeds address space")),
        }
    }
}
