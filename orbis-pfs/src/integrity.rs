//! HMAC verification on decrypted signed PFS images.
//! Format references: <https://github.com/maxton/LibOrbisPkg/tree/master/LibOrbisPkg/PFS>
use crate::{Pfs, image::Image};
use hmac::{Hmac, Mac};
pub use orbis_integrity::{Check, IntegrityReport, Reason, Status};
use sha2::Sha256;
use std::io;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

struct Verifier<'a, F, O> {
    image: &'a dyn Image,
    block_size: usize,
    key: [u8; 32],
    report: IntegrityReport,
    cancelled: F,
    observer: O,
    // Reused for leaf blocks; only indirect levels need their own buffers.
    scratch: Vec<u8>,
}

impl Pfs<'_> {
    /// Verifies signed header, inode-table and direct/indirect data blocks using
    /// the EKPFS-derived signing key. Unsigned images have no HMACs to check.
    /// Missing checks are explicit; this does not verify publisher signatures.
    pub fn verify_integrity(
        &self,
        ekpfs: Option<&[u8]>,
        cancelled: impl Fn() -> bool,
    ) -> io::Result<IntegrityReport> {
        self.verify_integrity_with(ekpfs, cancelled, |_| Ok(()))
    }

    /// Streams check locations and unavailable reasons without retaining a
    /// record per block. Stops on the first mismatch, cancellation or I/O error.
    pub fn verify_integrity_with(
        &self,
        ekpfs: Option<&[u8]>,
        cancelled: impl Fn() -> bool,
        mut observer: impl FnMut(Check<'_>) -> io::Result<()>,
    ) -> io::Result<IntegrityReport> {
        if cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let mut header = vec![0; 0x5a0];
        self.image.read_exact_at(0, &mut header)?;
        let mut report = IntegrityReport::default();
        if header[0x1c] & 1 == 0 {
            report.record(
                "PFS image",
                Status::NotApplicable,
                Some(Reason::UnsignedImage),
                0,
                &mut observer,
            )?;
            return Ok(report);
        }
        let Some(ekpfs) = ekpfs.filter(|k| k.len() == 32) else {
            report.record(
                "PFS image",
                Status::Unavailable,
                Some(Reason::MissingKey),
                1,
                &mut observer,
            )?;
            return Ok(report);
        };
        let mut derive = Hmac::<Sha256>::new_from_slice(ekpfs).unwrap();
        derive.update(&2u32.to_le_bytes());
        derive.update(&header[0x370..0x380]);
        let mut verifier = Verifier {
            image: &*self.image,
            block_size: self.block_size as usize,
            key: derive.finalize().into_bytes().into(),
            report,
            cancelled,
            observer,
            scratch: vec![0; self.block_size as usize],
        };
        let stored = header[0x380..0x3a0].try_into().unwrap();
        header[0x380..0x3a0].fill(0);
        verifier.check("PFS header", &stored, &header)?;
        verifier.inode_table(&header)?;
        for (index, inode) in self.inodes.iter().enumerate() {
            let label = format!("inode {index}");
            let mut remaining = u64::from(inode.raw().blocks.get());
            for i in 0..12 {
                if remaining == 0 {
                    break;
                }
                let block = inode.direct_blocks[i];
                if i == 1 && block == u32::MAX {
                    verifier.unavailable(&label, Reason::ContiguousExtent, remaining as usize)?;
                    remaining = 0;
                    break;
                }
                verifier.leaf(
                    &label,
                    u64::from(block),
                    &inode.direct_sigs[i].unwrap_or([0; 32]),
                )?;
                remaining -= 1;
            }
            for depth in 1..=2 {
                if remaining == 0 {
                    break;
                }
                verifier.indirect(
                    &label,
                    inode.indirect_blocks[depth - 1],
                    &inode.indirect_sigs[depth - 1].unwrap_or([0; 32]),
                    depth,
                    &mut remaining,
                )?;
            }
            if remaining != 0 {
                return Err(invalid("unsupported PFS signature tree"));
            }
        }
        Ok(verifier.report)
    }
}

impl<F: Fn() -> bool, O: FnMut(Check<'_>) -> io::Result<()>> Verifier<'_, F, O> {
    fn check_cancelled(&self) -> io::Result<()> {
        if (self.cancelled)() {
            Err(io::Error::from(io::ErrorKind::Interrupted))
        } else {
            Ok(())
        }
    }

    fn unavailable(&mut self, label: &str, reason: Reason, count: usize) -> io::Result<()> {
        self.check_cancelled()?;
        self.report.record(
            label,
            Status::Unavailable,
            Some(reason),
            count,
            &mut self.observer,
        )
    }

    fn check(&mut self, label: &str, expected: &[u8; 32], data: &[u8]) -> io::Result<()> {
        self.check_cancelled()?;
        if *expected == [0; 32] {
            return self.unavailable(label, Reason::MissingDigest, 1);
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).unwrap();
        for chunk in data.chunks(1024 * 1024) {
            self.check_cancelled()?;
            mac.update(chunk);
        }
        if mac.verify_slice(expected).is_err() {
            self.report.record(
                label,
                Status::Failed,
                Some(Reason::DigestMismatch),
                1,
                &mut self.observer,
            )?;
            return Err(invalid(format!("HMAC mismatch: {label}")));
        }
        self.report
            .record(label, Status::Passed, None, 1, &mut self.observer)
    }

    fn read_block(&self, block: u64, data: &mut [u8]) -> io::Result<()> {
        self.check_cancelled()?;
        if block == 0 || block >= self.image.len() / self.block_size as u64 {
            return Err(invalid(format!(
                "PFS signature block {block} outside image"
            )));
        }
        self.image
            .read_exact_at(block * self.block_size as u64, data)
    }

    fn leaf(&mut self, label: &str, block: u64, sig: &[u8; 32]) -> io::Result<()> {
        let mut data = std::mem::take(&mut self.scratch);
        let result = self
            .read_block(block, &mut data)
            .and_then(|()| self.check(&format!("{label} data block {block}"), sig, &data));
        self.scratch = data;
        result
    }

    fn branch(&mut self, label: &str, block: u64, sig: &[u8; 32]) -> io::Result<Vec<u8>> {
        let mut data = vec![0; self.block_size];
        self.read_block(block, &mut data)?;
        self.check(&format!("{label} indirect block {block}"), sig, &data)?;
        Ok(data)
    }

    fn indirect(
        &mut self,
        label: &str,
        block: u32,
        sig: &[u8; 32],
        depth: usize,
        remaining: &mut u64,
    ) -> io::Result<()> {
        let data = self.branch(label, u64::from(block), sig)?;
        for slot in data.as_chunks::<36>().0 {
            if *remaining == 0 {
                break;
            }
            let sig = slot[..32].try_into().unwrap();
            let next = u32::from_le_bytes(slot[32..].try_into().unwrap());
            if depth == 1 {
                self.leaf(label, u64::from(next), &sig)?;
                *remaining -= 1;
            } else {
                self.indirect(label, next, &sig, depth - 1, remaining)?;
            }
        }
        Ok(())
    }

    fn inode_table(&mut self, header: &[u8]) -> io::Result<()> {
        // The header descriptor uses S64 (40-byte slots), even on S32 images.
        let total = u64::from_le_bytes(header[0x40..0x48].try_into().unwrap());
        let mut next = 1;
        for i in 0..total.min(12) as usize {
            let at = 0xb8 + i * 40;
            self.table_leaf(&header[at..at + 40], &mut next)?;
        }
        for depth in 1..=2 {
            if next > total {
                break;
            }
            let at = 0xb8 + (11 + depth) * 40;
            let slot = &header[at..at + 40];
            // An absent descriptor cannot locate the descendant signatures.
            if slot.iter().all(|b| *b == 0) {
                let capacity = (self.block_size / 40) as u64;
                let missing = (total - next + 1).min(capacity.pow(depth as u32));
                self.unavailable("inode table", Reason::MissingDigest, missing as usize)?;
                next += missing;
            } else {
                self.table_indirect(slot, depth, total, &mut next)?;
            }
        }
        if next <= total {
            self.unavailable(
                "inode table",
                Reason::UnsupportedLayout,
                (total - next + 1) as usize,
            )?;
        }
        Ok(())
    }

    fn table_leaf(&mut self, slot: &[u8], next: &mut u64) -> io::Result<()> {
        let sig = slot[..32].try_into().unwrap();
        let block = u64::from_le_bytes(slot[32..40].try_into().unwrap());
        // The parser reads the table sequentially. A nonzero MAC must cover
        // that exact block; authenticating a different block proves nothing.
        if sig != [0; 32] && block != *next {
            return Err(invalid(
                "PFS inode-table signature points to a different block",
            ));
        }
        self.leaf("inode table", *next, &sig)?;
        *next += 1;
        Ok(())
    }

    fn table_indirect(
        &mut self,
        slot: &[u8],
        depth: usize,
        total: u64,
        next: &mut u64,
    ) -> io::Result<()> {
        let sig = slot[..32].try_into().unwrap();
        let block = u64::from_le_bytes(slot[32..40].try_into().unwrap());
        if slot.iter().all(|b| *b == 0) {
            let missing =
                (total - *next + 1).min(((self.block_size / 40) as u64).pow(depth as u32));
            self.unavailable("inode table", Reason::MissingDigest, missing as usize)?;
            *next += missing;
            return Ok(());
        }
        let data = self.branch("inode table", block, &sig)?;
        for child in data.as_chunks::<40>().0 {
            if *next > total {
                break;
            }
            if depth == 1 {
                self.table_leaf(child, next)?;
            } else {
                self.table_indirect(child, depth - 1, total, next)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
