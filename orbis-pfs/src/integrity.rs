//! HMAC verification on decrypted signed PFS images.
//! Format references: <https://github.com/maxton/LibOrbisPkg/tree/master/LibOrbisPkg/PFS>
use crate::{Pfs, image::Image};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::io;

#[derive(Debug, Default)]
pub struct IntegrityReport {
    pub checked: usize,
    /// Missing keys/signatures or layouts without individual stored signatures.
    pub missing: usize,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

struct Verifier<'a, F> {
    image: &'a dyn Image,
    block_size: usize,
    key: [u8; 32],
    report: IntegrityReport,
    cancelled: F,
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
        let mut header = vec![0; 0x5a0];
        self.image.read_exact_at(0, &mut header)?;
        if header[0x1c] & 1 == 0 {
            return Ok(IntegrityReport::default());
        }
        let Some(ekpfs) = ekpfs.filter(|k| k.len() == 32) else {
            return Ok(IntegrityReport {
                checked: 0,
                missing: 1,
            });
        };
        let mut derive = Hmac::<Sha256>::new_from_slice(ekpfs).unwrap();
        derive.update(&2u32.to_le_bytes());
        derive.update(&header[0x370..0x380]);
        let mut verifier = Verifier {
            image: &*self.image,
            block_size: self.block_size as usize,
            key: derive.finalize().into_bytes().into(),
            report: IntegrityReport::default(),
            cancelled,
        };
        let stored: [u8; 32] = header[0x380..0x3a0].try_into().unwrap();
        header[0x380..0x3a0].fill(0);
        verifier.check("PFS header", &stored, &header)?;
        // The header's inode-table descriptor is S64 even on S32 images.
        let table_blocks = u64::from_le_bytes(header[0x40..0x48].try_into().unwrap());
        for i in 0..table_blocks.min(12) as usize {
            let at = 0xb8 + i * 40;
            let sig = header[at..at + 32].try_into().unwrap();
            let block = u64::from_le_bytes(header[at + 32..at + 40].try_into().unwrap());
            // Table blocks are read sequentially by the parser. Check that the
            // descriptor authenticates those same blocks, rather than another area.
            if sig != [0; 32] && block != i as u64 + 1 {
                return Err(invalid(
                    "PFS inode-table signature points to a different block",
                ));
            }
            verifier.block(i as u64 + 1, &sig)?;
        }
        // Do not silently certify images whose table descriptor needs indirect
        // S64 addressing, which this S32 reader does not yet interpret.
        verifier.report.missing += table_blocks.saturating_sub(12) as usize;
        for inode in &self.inodes {
            let mut remaining = u64::from(inode.raw().blocks.get());
            for i in 0..12 {
                if remaining == 0 {
                    break;
                }
                let block = inode.direct_blocks[i];
                if block == u32::MAX {
                    // Contiguous extents have no per-block signatures for the
                    // remainder. Package-level image digests are still checked.
                    verifier.report.missing += remaining as usize;
                    remaining = 0;
                    break;
                }
                verifier.block(u64::from(block), &inode.direct_sigs[i].unwrap_or([0; 32]))?;
                remaining -= 1;
            }
            for depth in 1..=2 {
                if remaining == 0 {
                    break;
                }
                verifier.indirect(
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

impl<F: Fn() -> bool> Verifier<'_, F> {
    fn check(&mut self, label: &str, expected: &[u8; 32], data: &[u8]) -> io::Result<()> {
        if (self.cancelled)() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if *expected == [0; 32] {
            self.report.missing += 1;
        } else {
            let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).unwrap();
            mac.update(data);
            mac.verify_slice(expected)
                .map_err(|_| invalid(format!("HMAC mismatch: {label}")))?;
            self.report.checked += 1;
        }
        Ok(())
    }

    fn block(&mut self, block: u64, sig: &[u8; 32]) -> io::Result<Vec<u8>> {
        if (self.cancelled)() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if block == 0 || block >= self.image.len() / self.block_size as u64 {
            return Err(invalid(format!(
                "PFS signature block {block} outside image"
            )));
        }
        let mut data = vec![0; self.block_size];
        self.image
            .read_exact_at(block * self.block_size as u64, &mut data)?;
        self.check(&format!("PFS block {block}"), sig, &data)?;
        Ok(data)
    }

    fn indirect(
        &mut self,
        block: u32,
        sig: &[u8; 32],
        depth: usize,
        remaining: &mut u64,
    ) -> io::Result<()> {
        let data = self.block(u64::from(block), sig)?;
        for slot in data.as_chunks::<36>().0 {
            if *remaining == 0 {
                break;
            }
            let sig = slot[..32].try_into().unwrap();
            let next = u32::from_le_bytes(slot[32..].try_into().unwrap());
            if depth == 1 {
                self.block(u64::from(next), &sig)?;
                *remaining -= 1;
            } else {
                self.indirect(next, &sig, depth - 1, remaining)?;
            }
        }
        Ok(())
    }
}
