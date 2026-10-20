use flate2::FlushDecompress;
use std::cmp::min;
use std::io::{self, ErrorKind};
use zerocopy::{
    FromBytes, Immutable, KnownLayout,
    little_endian::{U32, U64},
};

use crate::image::Image;
use snafu::{Snafu, ensure};

/// PFSC header (48 bytes).
#[derive(Clone, Copy, FromBytes, KnownLayout, Immutable)]
#[repr(C)]
struct PfscHeader {
    /// 0x00: Magic bytes "PFSC"
    magic: [u8; 4],
    /// 0x04: Unknown
    _unknown_04: U32,
    /// 0x08: Unknown
    _unknown_08: U32,
    /// 0x0C: Compressed block size
    block_size: U32,
    /// 0x10: Original block size
    block_size2: U64,
    /// 0x18: Offset to block mapping table
    block_offsets: U64,
    /// 0x20: Unknown
    _unknown_20: U64,
    /// 0x28: Original (uncompressed) data length
    data_length: U64,
}

const PFSC_MAGIC: &[u8; 4] = b"PFSC";

/// Errors when opening a PFSC compressed file.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum OpenError {
    #[snafu(display("invalid or unsupported PFSC block size"))]
    InvalidBlockSize,

    #[snafu(display("invalid PFSC block mapping"))]
    InvalidBlockMapping,

    #[snafu(display("cannot allocate PFSC block mapping"))]
    Allocate {
        source: std::collections::TryReserveError,
    },
    #[snafu(display("i/o failed"))]
    IoFailed { source: std::io::Error },

    #[snafu(display("data too small"))]
    TooSmall,

    #[snafu(display("invalid magic"))]
    InvalidMagic,

    #[snafu(display("cannot read block mapping"))]
    ReadBlockMappingFailed { source: std::io::Error },
}

/// A decompressing [`Image`] adapter for PFSC-compressed files.
///
/// Each PFSC block is independently compressed, so `read_at` at any offset
/// only needs to decompress one block (or two if straddling a boundary).
/// All state is local to each call — no shared mutable state, naturally
/// thread-safe.
///
/// Created via [`PfscImage::open()`].
///
/// # Example
///
/// ```no_run
/// use orbis_pfs::image::Image;
/// use orbis_pfs::pfsc::PfscImage;
///
/// # fn example(source: impl Image) -> Result<(), Box<dyn std::error::Error>> {
/// let pfsc = PfscImage::open(source)?;
/// let pfs = orbis_pfs::open_image(pfsc)?;
/// # Ok(())
/// # }
/// ```
pub struct PfscImage<I: Image> {
    source: I,
    block_size: u32,
    original_block_size: u64,
    compressed_blocks: Vec<u64>,
    original_size: u64,
}

impl<I: Image> std::fmt::Debug for PfscImage<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PfscImage")
            .field("block_size", &self.block_size)
            .field("original_block_size", &self.original_block_size)
            .field("original_size", &self.original_size)
            .finish_non_exhaustive()
    }
}

impl<I: Image> PfscImage<I> {
    /// Opens a PFSC-compressed image from an underlying [`Image`] source.
    ///
    /// Reads the PFSC header and block offset table at construction time.
    pub fn open(source: I) -> Result<Self, OpenError> {
        // Read header.
        let mut header_buf = [0u8; size_of::<PfscHeader>()];

        source.read_exact_at(0, &mut header_buf).map_err(|e| {
            if e.kind() == ErrorKind::UnexpectedEof {
                OpenError::TooSmall
            } else {
                OpenError::IoFailed { source: e }
            }
        })?;

        let header =
            PfscHeader::read_from_bytes(&header_buf).expect("header buffer is correctly sized");

        ensure!(&header.magic == PFSC_MAGIC, InvalidMagicSnafu);

        let block_size = header.block_size.get();
        let original_block_size = header.block_size2.get();
        let block_offsets_offset = header.block_offsets.get();
        let original_size = header.data_length.get();

        ensure!(
            block_size != 0
                && block_size.is_power_of_two()
                && block_size <= 32 * 1024 * 1024
                && original_block_size == u64::from(block_size),
            InvalidBlockSizeSnafu
        );
        // Read block offsets.
        let count = original_size
            .div_ceil(original_block_size)
            .checked_add(1)
            .ok_or(OpenError::InvalidBlockMapping)?;
        let table_end = count
            .checked_mul(8)
            .and_then(|size| block_offsets_offset.checked_add(size))
            .ok_or(OpenError::InvalidBlockMapping)?;
        ensure!(
            block_offsets_offset >= size_of::<PfscHeader>() as u64 && table_end <= source.len(),
            InvalidBlockMappingSnafu
        );
        let count = usize::try_from(count).map_err(|_| OpenError::InvalidBlockMapping)?;
        let mut compressed_blocks = Vec::new();
        compressed_blocks
            .try_reserve_exact(count)
            .map_err(|source| OpenError::Allocate { source })?;
        // Decode explicitly so the table is portable to big-endian hosts too.
        let mut table_buf = [0; 8192];
        for i in (0..count).step_by(table_buf.len() / 8) {
            let bytes = min(count - i, table_buf.len() / 8) * 8;
            source
                .read_exact_at(block_offsets_offset + i as u64 * 8, &mut table_buf[..bytes])
                .map_err(|source| OpenError::ReadBlockMappingFailed { source })?;
            for value in table_buf[..bytes].as_chunks::<8>().0 {
                compressed_blocks.push(u64::from_le_bytes(*value));
            }
        }
        for offsets in compressed_blocks.windows(2) {
            let size = offsets[1]
                .checked_sub(offsets[0])
                .ok_or(OpenError::InvalidBlockMapping)?;
            // Larger ranges encode sparse blocks; no source bytes are read for them.
            ensure!(
                size > original_block_size
                    || (offsets[0] >= table_end && offsets[1] <= source.len()),
                InvalidBlockMappingSnafu
            );
        }

        Ok(Self {
            source,
            block_size,
            original_block_size,
            compressed_blocks,
            original_size,
        })
    }

    /// Returns the decompressed size of the file.
    #[must_use]
    pub fn decompressed_len(&self) -> u64 {
        self.original_size
    }

    /// Decompresses a single PFSC block into `out`.
    ///
    /// `out` must be exactly `self.block_size` bytes.
    fn decompress_block(&self, num: u64, out: &mut [u8]) -> io::Result<()> {
        debug_assert_eq!(out.len(), self.block_size as usize);

        // Get compressed block range.
        let end = match self.compressed_blocks.get(num as usize + 1) {
            Some(&v) => v,
            None => return Err(io::Error::from(ErrorKind::InvalidInput)),
        };

        let offset = self.compressed_blocks[num as usize];
        let size = end - offset;

        match size.cmp(&self.original_block_size) {
            std::cmp::Ordering::Less => {
                // Read compressed data.
                let mut compressed_buf = vec![0u8; size as usize];
                self.source.read_exact_at(offset, &mut compressed_buf)?;

                // Decompress.
                let mut deflate = flate2::Decompress::new(true);

                let status = match deflate.decompress(&compressed_buf, out, FlushDecompress::Finish)
                {
                    Ok(v) => v,
                    Err(e) => return Err(io::Error::other(e)),
                };

                let valid = min(
                    self.original_block_size,
                    self.original_size - num * self.original_block_size,
                );
                if status != flate2::Status::StreamEnd || deflate.total_out() < valid {
                    return Err(io::Error::other(format!(
                        "invalid data on PFSC block #{}",
                        num
                    )));
                }
            }

            std::cmp::Ordering::Equal => {
                // Uncompressed block — read directly.
                self.source.read_exact_at(offset, out)?;
            }

            std::cmp::Ordering::Greater => {
                // Sparse / zero block.
                out.fill(0);
            }
        }

        Ok(())
    }
}

impl<I: Image> Image for PfscImage<I> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || offset >= self.original_size {
            return Ok(0);
        }

        let block_size = self.block_size as u64;
        let mut copied = 0usize;
        let mut pos = offset;
        let mut block_buf = vec![0u8; self.block_size as usize];

        while copied < buf.len() && pos < self.original_size {
            // Determine which PFSC block and offset within it.
            let block_index = pos / block_size;
            let offset_in_block = (pos % block_size) as usize;

            // Decompress the block.
            self.decompress_block(block_index, &mut block_buf)?;

            // Trim the last block if it extends past the original size.
            let valid_in_block =
                min(block_size, self.original_size - block_index * block_size) as usize;

            // Copy the relevant portion to the output buffer.
            let available = valid_in_block - offset_in_block;
            let n = min(available, buf.len() - copied);

            buf[copied..copied + n]
                .copy_from_slice(&block_buf[offset_in_block..offset_in_block + n]);

            copied += n;
            pos += n as u64;
        }

        Ok(copied)
    }

    fn len(&self) -> u64 {
        self.original_size
    }
}
