use super::*;
use crate::directory::DirEntry;
use crate::image::{Image, UnencryptedSlice};
use crate::pfsc::PfscImage;
use flate2::{Compression, write::ZlibEncoder};
use std::io::{self, Read, Seek, SeekFrom, Write};

const BLOCK_SIZE: usize = 4096;
const EKPFS: [u8; 32] = [0x43; 32];

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn inode_offset(index: usize, signed: bool) -> usize {
    BLOCK_SIZE + index * if signed { 712 } else { 168 }
}

// Independent signed-image encoder, including single and double indirect blocks.
fn authenticated_pfs(count: usize, encrypted: bool) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut bytes = pfs_bytes(&vec![0x5a; count * BLOCK_SIZE], true);
    bytes[0x370..0x380].fill(0x17);
    if encrypted {
        bytes[0x1c] |= 4;
    }
    let mut key = <Hmac<Sha256> as Mac>::new_from_slice(&EKPFS).unwrap();
    key.update(&[2, 0, 0, 0]);
    key.update(&[0x17; 16]);
    let key = key.finalize().into_bytes();
    let sign = |b: &mut [u8], slot: usize, block: usize| {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key).unwrap();
        mac.update(&b[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE]);
        b[slot..slot + 32].copy_from_slice(&mac.finalize().into_bytes());
        put_u32(b, slot + 32, block as u32);
    };
    let inode = inode_offset(2, true);
    let capacity = BLOCK_SIZE / 36;
    for i in 0..count.min(12) {
        sign(&mut bytes, inode + 100 + i * 36, 4 + i);
    }
    if count > 12 {
        let single = bytes.len() / BLOCK_SIZE;
        bytes.resize(bytes.len() + BLOCK_SIZE, 0);
        for i in 0..(count - 12).min(capacity) {
            sign(&mut bytes, single * BLOCK_SIZE + i * 36, 16 + i);
        }
        sign(&mut bytes, inode + 100 + 12 * 36, single);
        if count > 12 + capacity {
            let double = bytes.len() / BLOCK_SIZE;
            bytes.resize(bytes.len() + 2 * BLOCK_SIZE, 0);
            for i in 0..count - 12 - capacity {
                sign(
                    &mut bytes,
                    (double + 1) * BLOCK_SIZE + i * 36,
                    16 + capacity + i,
                );
            }
            sign(&mut bytes, double * BLOCK_SIZE, double + 1);
            sign(&mut bytes, inode + 100 + 13 * 36, double);
        }
    }
    for i in 0..2 {
        sign(&mut bytes, inode_offset(i, true) + 100, 2 + i);
    }
    // Header S64 descriptor authenticates the inode-table block.
    sign(&mut bytes, 0xb8, 1);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key).unwrap();
    mac.update(&bytes[..0x5a0]);
    bytes[0x380..0x3a0].copy_from_slice(&mac.finalize().into_bytes());
    if encrypted {
        encrypt_pfs(&mut bytes);
    }
    bytes
}

#[test]
fn integrity_verifies_direct_indirect_and_encrypted_pfs_blocks() {
    for count in [1, 12, 13, 12 + BLOCK_SIZE / 36 + 1] {
        for encrypted in [false, true] {
            let bytes = authenticated_pfs(count, encrypted);
            let pfs = open_slice(&bytes, Some(&EKPFS)).unwrap();
            let report = pfs.verify_integrity(Some(&EKPFS), || false).unwrap();
            assert_eq!(report.missing, 0, "count {count}, encrypted {encrypted}");
            assert!(report.checked >= count + 4);
            assert_eq!(
                pfs.verify_integrity(Some(&EKPFS), || true)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::Interrupted
            );
            // Readability still succeeds after changing data, integrity must not.
            let mut bad = bytes.clone();
            bad[4 * BLOCK_SIZE + 127] ^= 1;
            let pfs = open_slice(&bad, Some(&EKPFS)).unwrap();
            assert!(
                pfs.verify_integrity(Some(&EKPFS), || false)
                    .unwrap_err()
                    .to_string()
                    .contains("HMAC mismatch")
            );
            if count > 12 {
                let mut bad = bytes.clone();
                bad[(4 + count - 1) * BLOCK_SIZE + 127] ^= 1;
                let pfs = open_slice(&bad, Some(&EKPFS)).unwrap();
                assert!(pfs.verify_integrity(Some(&EKPFS), || false).is_err());
            }
        }
    }
}

#[test]
fn integrity_never_certifies_absent_macs_or_missing_keys() {
    let bytes = pfs_bytes(b"hello", true);
    let pfs = open_slice(&bytes, None).unwrap();
    assert!(
        pfs.verify_integrity(Some(&EKPFS), || false)
            .unwrap()
            .missing
            > 0
    );
    assert_eq!(pfs.verify_integrity(None, || false).unwrap().missing, 1);
    let bytes = authenticated_pfs(1, false);
    for at in [0x380, 0xb8, BLOCK_SIZE + 100] {
        let mut bad = bytes.clone();
        bad[at] ^= 1;
        let pfs = open_slice(&bad, None).unwrap();
        assert!(pfs.verify_integrity(Some(&EKPFS), || false).is_err());
    }
    assert!(
        open_slice(&bytes, None)
            .unwrap()
            .verify_integrity(Some(&[0x99; 32]), || false)
            .is_err()
    );
    let unsigned = pfs_bytes(b"hello", false);
    let report = open_slice(&unsigned, None)
        .unwrap()
        .verify_integrity(None, || false)
        .unwrap();
    assert_eq!((report.checked, report.missing), (0, 0));
}

fn pointer_offset(index: usize, signed: bool) -> usize {
    100 + if signed { index * 36 + 32 } else { index * 4 }
}

fn dirent(bytes: &mut [u8], inode: u32, kind: u32, name: &[u8]) {
    put_u32(bytes, 0, inode);
    put_u32(bytes, 4, kind);
    put_u32(bytes, 8, name.len() as u32);
    put_u32(bytes, 12, (16 + name.len()) as u32);
    bytes[16..16 + name.len()].copy_from_slice(name);
}

/// An independently encoded image: super-root -> uroot -> payload.bin.
fn pfs_bytes(contents: &[u8], signed: bool) -> Vec<u8> {
    let file_blocks = contents.len().div_ceil(BLOCK_SIZE);
    let mut bytes = vec![0; (4 + file_blocks) * BLOCK_SIZE];
    put_u64(&mut bytes, 0, 1);
    put_u64(&mut bytes, 8, 20130315);
    bytes[0x1c] = u8::from(signed);
    put_u32(&mut bytes, 0x20, BLOCK_SIZE as u32);
    put_u64(&mut bytes, 0x28, (4 + file_blocks) as u64);
    put_u64(&mut bytes, 0x30, 3);
    put_u64(&mut bytes, 0x40, 1);
    for index in 0..3 {
        let base = inode_offset(index, signed);
        let mode: u16 = if index == 2 { 0x8000 } else { 0x4000 };
        bytes[base..base + 2].copy_from_slice(&mode.to_le_bytes());
        let size = if index == 2 {
            contents.len()
        } else {
            BLOCK_SIZE
        };
        put_u64(&mut bytes, base + 8, size as u64);
        put_u64(&mut bytes, base + 16, size as u64);
        put_u32(
            &mut bytes,
            base + 96,
            if index == 2 { file_blocks as u32 } else { 1 },
        );
        put_u32(
            &mut bytes,
            base + pointer_offset(0, signed),
            (2 + index) as u32,
        );
        put_u32(&mut bytes, base + pointer_offset(1, signed), u32::MAX);
    }
    dirent(&mut bytes[2 * BLOCK_SIZE..], 1, 3, b"uroot");
    dirent(&mut bytes[3 * BLOCK_SIZE..], 2, 2, b"payload.bin");
    bytes[4 * BLOCK_SIZE..4 * BLOCK_SIZE + contents.len()].copy_from_slice(contents);
    bytes
}

fn payload<'a>(pfs: &Arc<Pfs<'a>>) -> file::File<'a> {
    let DirEntry::Directory(root) = pfs.root().open().unwrap().remove(b"uroot").unwrap() else {
        panic!("uroot must be a directory");
    };
    let DirEntry::File(file) = root.open().unwrap().remove(b"payload.bin").unwrap() else {
        panic!("payload.bin must be a file");
    };
    file
}

fn encrypt_pfs(bytes: &mut [u8]) {
    bytes[0x1c] |= 4;
    let seed = [0x17; 16];
    bytes[0x370..0x380].copy_from_slice(&seed);
    let (data_key, tweak_key) = image::get_xts_keys(&EKPFS, &seed);
    let cipher = Xts128::<Aes128>::new(
        Aes128::new((&data_key).into()),
        Aes128::new((&tweak_key).into()),
    );
    for (index, sector) in bytes
        .as_chunks_mut::<BLOCK_SIZE>()
        .0
        .iter_mut()
        .enumerate()
        .skip(1)
    {
        cipher.encrypt_sector(sector, xts_mode::get_tweak_default(index as u128));
    }
}

fn pfsc_bytes(contents: &[u8], compressed: bool) -> Vec<u8> {
    let count = contents.len().div_ceil(BLOCK_SIZE);
    let table_end = 48 + (count + 1) * 8;
    let mut bytes = vec![0; table_end];
    bytes[..4].copy_from_slice(b"PFSC");
    put_u32(&mut bytes, 12, BLOCK_SIZE as u32);
    put_u64(&mut bytes, 16, BLOCK_SIZE as u64);
    put_u64(&mut bytes, 24, 48);
    put_u64(&mut bytes, 40, contents.len() as u64);
    for (index, block) in contents.chunks(BLOCK_SIZE).enumerate() {
        let offset = bytes.len() as u64;
        put_u64(&mut bytes, 48 + index * 8, offset);
        if compressed {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(block).unwrap();
            let encoded = encoder.finish().unwrap();
            assert!(encoded.len() < BLOCK_SIZE);
            bytes.extend_from_slice(&encoded);
        } else {
            bytes.extend_from_slice(block);
            bytes.resize(bytes.len() + BLOCK_SIZE - block.len(), 0);
        }
    }
    let end = bytes.len() as u64;
    put_u64(&mut bytes, 48 + count * 8, end);
    bytes
}

#[test]
fn unsigned_and_signed_files_support_positional_and_cursor_reads() {
    let contents: Vec<u8> = (0..BLOCK_SIZE + 29).map(|i| (i % 251) as u8).collect();
    for signed in [false, true] {
        let bytes = pfs_bytes(&contents, signed);
        for slice_backed in [false, true] {
            let pfs = if slice_backed {
                open_slice(&bytes, None).unwrap()
            } else {
                open_image(UnencryptedSlice::new(&bytes)).unwrap()
            };
            assert_eq!(pfs.inode_count(), 3);
            let file = payload(&pfs);
            assert_eq!(file.len(), contents.len() as u64);
            assert_eq!(file.as_slice(), slice_backed.then_some(contents.as_slice()));
            let mut tail = [0xcc; 64];
            assert_eq!(
                file.read_at((BLOCK_SIZE - 7) as u64, &mut tail).unwrap(),
                36
            );
            assert_eq!(&tail[..36], &contents[BLOCK_SIZE - 7..]);
            assert_eq!(&tail[36..], &[0xcc; 28]);
            assert_eq!(file.read_at(u64::MAX, &mut tail).unwrap(), 0);
            let mut reader = file.reader();
            assert_eq!(reader.seek(SeekFrom::End(-5)).unwrap(), file.len() - 5);
            let mut last = Vec::new();
            reader.read_to_end(&mut last).unwrap();
            assert_eq!(last, contents[contents.len() - 5..]);
            assert_eq!(reader.read(&mut tail).unwrap(), 0);
        }
    }
}

#[test]
fn encrypted_images_keep_signed_and_unsigned_file_reads_working() {
    let contents = vec![0x5a; BLOCK_SIZE + 9];
    for signed in [false, true] {
        let mut bytes = pfs_bytes(&contents, signed);
        encrypt_pfs(&mut bytes);
        assert!(matches!(
            open_slice(&bytes, None),
            Err(OpenSliceError::EmptyEkpfs)
        ));
        assert!(matches!(
            open_image(UnencryptedSlice::new(&bytes)),
            Err(OpenImageError::UnsupportedMode { .. })
        ));
        let pfs = open_slice(&bytes, Some(&EKPFS)).unwrap();
        let file = payload(&pfs);
        assert!(file.as_slice().is_none());
        let mut actual = Vec::new();
        file.reader().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, contents);
    }
}

#[test]
fn empty_files_have_no_blocks_and_read_as_eof() {
    let bytes = pfs_bytes(&[], false);
    let pfs = open_slice(&bytes, None).unwrap();
    let file = payload(&pfs);
    assert!(file.is_empty());
    assert_eq!(file.as_slice(), Some(&[][..]));
    assert_eq!(file.read_at(0, &mut [0; 1]).unwrap(), 0);
}

#[test]
fn indirect_block_maps_read_unsigned_and_signed_pointers() {
    for signed in [false, true] {
        let entry_size = if signed { 36 } else { 4 };
        let value_offset = if signed { 32 } else { 0 };
        let entries_per_block = BLOCK_SIZE / entry_size;
        for count in [13, 12 + entries_per_block + 1] {
            let contents: Vec<u8> = (0..count * BLOCK_SIZE)
                .map(|i| ((i / BLOCK_SIZE) % 251) as u8)
                .collect();
            let mut bytes = pfs_bytes(&contents, signed);
            let inode = inode_offset(2, signed);
            for index in 0..12 {
                put_u32(
                    &mut bytes,
                    inode + pointer_offset(index, signed),
                    (4 + index) as u32,
                );
            }
            let indirect = bytes.len();
            put_u32(
                &mut bytes,
                inode + pointer_offset(12, signed),
                (indirect / BLOCK_SIZE) as u32,
            );
            let double_indirect = count > 12 + entries_per_block;
            bytes.resize(
                indirect + if double_indirect { 3 } else { 1 } * BLOCK_SIZE,
                0,
            );
            for index in 0..(count - 12).min(entries_per_block) {
                put_u32(
                    &mut bytes,
                    indirect + index * entry_size + value_offset,
                    (16 + index) as u32,
                );
            }
            let last_pointer = if double_indirect {
                put_u32(
                    &mut bytes,
                    inode + pointer_offset(13, signed),
                    (indirect / BLOCK_SIZE + 1) as u32,
                );
                put_u32(
                    &mut bytes,
                    indirect + BLOCK_SIZE + value_offset,
                    (indirect / BLOCK_SIZE + 2) as u32,
                );
                let pointer = indirect + 2 * BLOCK_SIZE + value_offset;
                put_u32(&mut bytes, pointer, (16 + entries_per_block) as u32);
                pointer
            } else {
                indirect + value_offset
            };
            let pfs = open_slice(&bytes, None).unwrap();
            let file = payload(&pfs);
            assert!(file.as_slice().is_none());
            let mut actual = Vec::new();
            file.reader().read_to_end(&mut actual).unwrap();
            assert_eq!(actual, contents);

            for pointer in [last_pointer, inode + pointer_offset(12, signed)] {
                let mut invalid = bytes.clone();
                put_u32(&mut invalid, pointer, u32::MAX);
                assert!(matches!(
                    open_slice(&invalid, None),
                    Err(OpenSliceError::Open {
                        source: OpenError::LoadBlockMapFailed { .. }
                    })
                ));
            }
        }
    }
}

#[test]
fn invalid_pfs_block_sizes_are_rejected_before_allocation() {
    for size in [0, 1, 2048, 4097, 32768, 64 * 1024 * 1024, u32::MAX] {
        let mut bytes = pfs_bytes(b"contents", false);
        put_u32(&mut bytes, 0x20, size);
        assert!(
            matches!(
                open_slice(&bytes, None),
                Err(OpenSliceError::Open {
                    source: OpenError::InvalidBlockSize
                })
            ),
            "block size {size}"
        );
    }
}

#[test]
fn malformed_inode_tables_and_unsupported_modes_return_errors() {
    for (field, value) in [
        (0x30, u64::MAX),
        (0x30, 25),
        (0x40, 0),
        (0x40, 5),
        (0x40, u64::MAX),
        (0x48, 3),
    ] {
        let mut bytes = pfs_bytes(b"contents", false);
        put_u64(&mut bytes, field, value);
        assert!(
            open_slice(&bytes, None).is_err(),
            "field {field:#x}, value {value}"
        );
    }
    let mut signed = pfs_bytes(b"contents", true);
    put_u64(&mut signed, 0x30, 6);
    assert!(matches!(
        open_slice(&signed, None),
        Err(OpenSliceError::Open {
            source: OpenError::InvalidInodeTable
        })
    ));
    let mut bytes = pfs_bytes(b"contents", false);
    bytes[0x1c] = 2;
    assert!(matches!(
        open_slice(&bytes, None),
        Err(OpenSliceError::Open {
            source: OpenError::UnsupportedMode { .. }
        })
    ));
    assert!(open_slice(&bytes[..0x370], None).is_err());
}

#[test]
fn invalid_inode_sizes_counts_and_block_addresses_return_errors() {
    let base = inode_offset(2, false);
    for (offset, value) in [(96, 0), (96, u32::MAX), (100, u32::MAX), (100, 5)] {
        let mut bytes = pfs_bytes(b"contents", false);
        put_u32(&mut bytes, base + offset, value);
        assert!(matches!(
            open_slice(&bytes, None),
            Err(OpenSliceError::Open {
                source: OpenError::LoadBlockMapFailed { .. }
            })
        ));
    }
    let mut bytes = pfs_bytes(b"contents", false);
    put_u64(&mut bytes, base + 8, u64::MAX);
    assert!(open_slice(&bytes, None).is_err());
    let mut bytes = pfs_bytes(b"contents", false);
    put_u32(&mut bytes, base + 104, 0);
    put_u32(&mut bytes, base + 100, 5);
    assert!(open_slice(&bytes, None).is_err());
}

#[test]
fn malformed_directory_entries_return_errors_without_panics() {
    for (offset, value) in [
        (12, 1),
        (12, 16),
        (12, 4097),
        (8, u32::MAX),
        (0, 3),
        (4, 99),
    ] {
        let mut bytes = pfs_bytes(b"contents", false);
        put_u32(&mut bytes, 2 * BLOCK_SIZE + offset, value);
        let pfs = open_slice(&bytes, None).unwrap();
        assert!(
            pfs.root().open().is_err(),
            "dirent field {offset}, value {value}"
        );
    }
}

#[test]
fn pfsc_reads_raw_and_zlib_partial_blocks_and_eof() {
    for length in [
        0,
        1,
        BLOCK_SIZE - 1,
        BLOCK_SIZE,
        BLOCK_SIZE + 1,
        2 * BLOCK_SIZE + 29,
    ] {
        let contents: Vec<u8> = (0..length).map(|i| b'A' + (i / BLOCK_SIZE) as u8).collect();
        for compressed in [false, true] {
            let bytes = pfsc_bytes(&contents, compressed);
            let image = PfscImage::open(UnencryptedSlice::new(&bytes)).unwrap();
            assert_eq!(image.len(), length as u64);
            assert_eq!(image.decompressed_len(), length as u64);
            let mut actual = vec![0xcc; length + 7];
            assert_eq!(image.read_at(0, &mut actual).unwrap(), length);
            assert_eq!(&actual[..length], contents);
            assert_eq!(&actual[length..], &[0xcc; 7]);
            assert_eq!(image.read_at(length as u64, &mut actual).unwrap(), 0);
            assert_eq!(image.read_at(u64::MAX, &mut actual).unwrap(), 0);
            assert_eq!(image.read_at(0, &mut []).unwrap(), 0);
            if length > BLOCK_SIZE {
                let mut boundary = [0; 2];
                image
                    .read_exact_at((BLOCK_SIZE - 1) as u64, &mut boundary)
                    .unwrap();
                assert_eq!(boundary, [b'A', b'B']);
            }
        }
    }
}

#[test]
fn pfsc_sparse_blocks_return_zeroes_without_reading_payload() {
    for count in [1, 1025] {
        // 1025 blocks require more than one buffered offset-table read.
        let mut bytes = pfsc_bytes(&[], false);
        let table_end = 48 + (count + 1) * 8;
        bytes.resize(table_end, 0);
        put_u64(&mut bytes, 40, (count * BLOCK_SIZE) as u64);
        for index in 0..=count {
            put_u64(
                &mut bytes,
                48 + index * 8,
                (table_end + index * (BLOCK_SIZE + 1)) as u64,
            );
        }
        let image = PfscImage::open(UnencryptedSlice::new(&bytes)).unwrap();
        for offset in [0, (count - 1) * BLOCK_SIZE] {
            let mut actual = vec![0xff; BLOCK_SIZE];
            image.read_exact_at(offset as u64, &mut actual).unwrap();
            assert_eq!(actual, vec![0; BLOCK_SIZE]);
        }
    }
}

#[test]
fn pfsc_rejects_zero_mismatched_and_oversized_block_sizes() {
    for (block, original) in [
        (0, 4096),
        (4096, 0),
        (4096, 8192),
        (4095, 4095),
        (64 * 1024 * 1024, 64 * 1024 * 1024),
    ] {
        let mut bytes = pfsc_bytes(b"content", false);
        put_u32(&mut bytes, 12, block);
        put_u64(&mut bytes, 16, original);
        assert!(matches!(
            PfscImage::open(UnencryptedSlice::new(&bytes)),
            Err(pfsc::OpenError::InvalidBlockSize)
        ));
    }
}

#[test]
fn pfsc_rejects_overflowing_truncated_reversed_and_overlapping_mappings() {
    for (field, value) in [
        (24, 0),
        (24, u64::MAX),
        (40, u64::MAX),
        (48, 63),
        (48, u64::MAX),
        (56, 63),
        (56, 0),
    ] {
        let mut bytes = pfsc_bytes(b"content", true);
        put_u64(&mut bytes, field, value);
        assert!(
            PfscImage::open(UnencryptedSlice::new(&bytes)).is_err(),
            "field {field}, value {value}"
        );
    }
    let bytes = pfsc_bytes(b"content", false);
    assert!(PfscImage::open(UnencryptedSlice::new(&bytes[..bytes.len() - 1])).is_err());
    assert!(PfscImage::open(UnencryptedSlice::new(&bytes[..60])).is_err());
    assert!(matches!(
        PfscImage::open(UnencryptedSlice::new(b"PFSC")),
        Err(pfsc::OpenError::TooSmall)
    ));
    let mut bytes = bytes;
    bytes[0] = b'X';
    assert!(matches!(
        PfscImage::open(UnencryptedSlice::new(&bytes)),
        Err(pfsc::OpenError::InvalidMagic)
    ));
}

#[test]
fn pfsc_rejects_corrupt_or_short_zlib_payloads() {
    let mut corrupt = pfsc_bytes(&vec![b'A'; BLOCK_SIZE], true);
    corrupt[64] = 0;
    let image = PfscImage::open(UnencryptedSlice::new(&corrupt)).unwrap();
    assert!(image.read_at(0, &mut [0; 1]).is_err());
    let mut short = pfsc_bytes(b"A", true);
    put_u64(&mut short, 40, BLOCK_SIZE as u64);
    let image = PfscImage::open(UnencryptedSlice::new(&short)).unwrap();
    assert!(image.read_at(0, &mut [0; 1]).is_err());
}

#[test]
fn compressed_file_images_use_stored_size_when_decoding() {
    let contents = vec![b'A'; BLOCK_SIZE + 17];
    for compressed in [false, true] {
        let container = pfsc_bytes(&contents, compressed);
        let mut bytes = pfs_bytes(&container, false);
        let base = inode_offset(2, false);
        put_u32(&mut bytes, base + 4, 1);
        put_u64(&mut bytes, base + 8, contents.len() as u64);
        let pfs = open_slice(&bytes, None).unwrap();
        let file = payload(&pfs);
        assert!(file.is_compressed());
        assert!(file.as_slice().is_none());
        assert_eq!(file.compressed_len(), container.len() as u64);
        let mut reader = file.reader();
        assert_eq!(
            reader.seek(SeekFrom::End(0)).unwrap(),
            container.len() as u64
        );
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
        reader.seek(SeekFrom::End(-7)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, container[container.len() - 7..]);
        reader.seek(SeekFrom::Start(u64::MAX)).unwrap();
        assert_eq!(
            reader.seek(SeekFrom::Current(1)).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(reader.stream_position().unwrap(), u64::MAX);
        reader.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(
            reader.seek(SeekFrom::Current(-1)).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let stored = file.into_image();
        assert_eq!(stored.len(), container.len() as u64);
        let decoded = PfscImage::open(stored).unwrap();
        let mut actual = vec![0; contents.len()];
        decoded.read_exact_at(0, &mut actual).unwrap();
        assert_eq!(actual, contents);
    }
}

#[test]
fn pfs_handles_are_send_sync_and_allow_concurrent_reads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Pfs<'static>>();
    assert_send_sync::<file::File<'static>>();
    assert_send_sync::<file::PfsFileImage<'static>>();
    assert_send_sync::<PfscImage<UnencryptedSlice<'static>>>();
    let bytes = pfs_bytes(&vec![0x72; BLOCK_SIZE + 17], false);
    let pfs = open_slice(&bytes, None).unwrap();
    std::thread::scope(|scope| {
        for offset in [0, BLOCK_SIZE - 1, BLOCK_SIZE + 1] {
            let file = payload(&pfs);
            scope.spawn(move || {
                let mut data = [0; 16];
                assert_eq!(file.read_at(offset as u64, &mut data).unwrap(), data.len());
                assert_eq!(data, [0x72; 16]);
            });
        }
    });
}

#[test]
fn backing_image_read_failures_propagate_during_open() {
    struct FailingImage {
        bytes: Vec<u8>,
        fail_at: u64,
    }
    impl Image for FailingImage {
        fn read_at(&self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
            if offset >= self.fail_at {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "fixture read failure",
                ));
            }
            UnencryptedSlice::new(&self.bytes).read_at(offset, output)
        }
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
    }
    assert!(matches!(
        open_image(FailingImage {
            bytes: pfs_bytes(b"content", false),
            fail_at: BLOCK_SIZE as u64,
        }),
        Err(OpenImageError::Open {
            source: OpenError::ReadBlockFailed { .. }
        })
    ));
    assert!(matches!(
        PfscImage::open(FailingImage {
            bytes: pfsc_bytes(b"content", false),
            fail_at: 0,
        }),
        Err(pfsc::OpenError::IoFailed { .. })
    ));
    assert!(matches!(
        PfscImage::open(FailingImage {
            bytes: pfsc_bytes(b"content", false),
            fail_at: 48,
        }),
        Err(pfsc::OpenError::ReadBlockMappingFailed { .. })
    ));
    let image = PfscImage::open(FailingImage {
        bytes: pfsc_bytes(b"content", false),
        fail_at: 64,
    })
    .unwrap();
    assert_eq!(
        image.read_at(0, &mut [0; 1]).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
}
