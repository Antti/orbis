use super::*;

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

fn raw_pkg() -> Vec<u8> {
    let mut bytes = vec![0; 0x1100];
    put_u32(&mut bytes, 0, 0x7f434e54);
    put_u32(&mut bytes, 0x10, 1);
    put_u32(&mut bytes, 0x18, 0x1000);
    put_u32(&mut bytes, 0x1000, EntryId::ParamSfo.as_u32());
    put_u32(&mut bytes, 0x1010, 0x1020);
    put_u32(&mut bytes, 0x1014, 4);
    bytes[0x1020..0x1024].copy_from_slice(b"data");
    put_u64(&mut bytes, 0x410, 0x1020);
    put_u64(&mut bytes, 0x418, 4);
    bytes
}

// Exercise bounds independently of unrelated RSA key loading.
fn package(bytes: Vec<u8>) -> Pkg<Vec<u8>> {
    Pkg {
        header: PkgHeader::read(bytes.as_slice()).unwrap(),
        raw: bytes,
        entry_key3: Vec::new(),
        passcode_keys: None,
        ekpfs: vec![0x43; 32],
    }
}

#[test]
fn pfs_ranges_reject_overflow_and_out_of_bounds_offsets() {
    let pkg = package(raw_pkg());
    let pfs = pkg.get_pfs_image().unwrap();
    assert_eq!(pfs.data, b"data");
    assert_eq!(pfs.ekpfs, &[0x43; 32]);
    for (offset, size) in [(1, u64::MAX), (u64::MAX, 1), (0x10ff, 2), (0x1101, 0)] {
        let mut bytes = raw_pkg();
        put_u64(&mut bytes, 0x410, offset);
        put_u64(&mut bytes, 0x418, size);
        assert!(
            package(bytes).get_pfs_image().is_none(),
            "offset {offset}, size {size}"
        );
    }
    let mut bytes = raw_pkg();
    put_u64(&mut bytes, 0x410, 0x1100);
    put_u64(&mut bytes, 0x418, 0);
    assert!(package(bytes).get_pfs_image().unwrap().data.is_empty());
}

#[test]
fn entry_data_checks_payload_bounds() {
    let pkg = package(raw_pkg());
    let (entry, index) = pkg.find_entry(EntryId::ParamSfo).unwrap();
    assert_eq!(index, 0);
    assert_eq!(pkg.entry_data(&entry).unwrap(), b"data");
    for (offset, size) in [(0x10ff, 2), (u32::MAX, 1), (1, u32::MAX)] {
        let mut bytes = raw_pkg();
        put_u32(&mut bytes, 0x1010, offset);
        put_u32(&mut bytes, 0x1014, size);
        let pkg = package(bytes);
        let (entry, _) = pkg.find_entry(EntryId::ParamSfo).unwrap();
        assert!(matches!(
            pkg.entry_data(&entry),
            Err(EntryDataError::InvalidDataOffset)
        ));
    }
}

#[test]
fn truncated_entry_tables_and_missing_entries_return_errors() {
    let pkg = package(raw_pkg());
    assert!(matches!(
        pkg.find_entry(EntryId::Icon0Png),
        Err(FindEntryError::NotFound)
    ));
    let mut bytes = raw_pkg();
    put_u32(&mut bytes, 0x18, 0x10ff);
    let pkg = package(bytes);
    assert!(matches!(
        pkg.find_entry(EntryId::ParamSfo),
        Err(FindEntryError::InvalidOffset { num: 0 })
    ));
    assert!(pkg.entries().next().unwrap().is_err());
}

#[test]
fn encrypted_entries_reject_missing_keys_and_truncated_padding() {
    let mut bytes = raw_pkg();
    put_u32(&mut bytes, 0x1008, 0x80000000);
    put_u32(&mut bytes, 0x100c, 0x3000);
    put_u32(&mut bytes, 0x1010, 0x10ff);
    put_u32(&mut bytes, 0x1014, 1);
    let mut pkg = package(bytes);
    let (entry, _) = pkg.find_entry(EntryId::ParamSfo).unwrap();
    assert!(matches!(
        pkg.entry_data(&entry),
        Err(EntryDataError::NoDecryptionKey { key_index: 3 })
    ));
    pkg.entry_key3 = vec![0x42; 32];
    assert!(matches!(
        pkg.entry_data(&entry),
        Err(EntryDataError::InvalidDataOffset)
    ));
}

#[test]
fn metadata_remains_readable_without_decryption_keys() {
    let mut pkg = Pkg::parse(raw_pkg()).unwrap();
    assert_eq!(pkg.header().entry_count(), 1);
    let (entry, _) = pkg.find_entry(EntryId::ParamSfo).unwrap();
    assert_eq!(pkg.entry_data(&entry).unwrap(), b"data");
    assert!(pkg.load_entry_keys().is_err());
    assert!(pkg.load_pfs_key().is_err());
    assert_eq!(pkg.entry_data(&entry).unwrap(), b"data");
    assert!(Pkg::new(raw_pkg()).is_err());
}

#[test]
fn param_sfo_data_is_optional_and_leaves_parsing_to_the_caller() {
    let pkg = Pkg::parse(raw_pkg()).unwrap();
    assert_eq!(pkg.param_sfo_data().unwrap().unwrap(), b"data");

    let mut bytes = raw_pkg();
    put_u32(&mut bytes, 0x1000, EntryId::Icon0Png.as_u32());
    assert!(
        Pkg::parse(bytes)
            .unwrap()
            .param_sfo_data()
            .unwrap()
            .is_none()
    );
}

#[test]
fn param_sfo_data_checks_the_entire_table_before_reading_payloads() {
    let mut bytes = raw_pkg();
    put_u32(&mut bytes, 0x10, 2);
    put_u32(&mut bytes, 0x1010, u32::MAX);
    put_u32(&mut bytes, 0x1020, EntryId::ParamSfo.as_u32());
    assert!(matches!(
        Pkg::parse(bytes).unwrap().param_sfo_data(),
        Err(ParamSfoError::DuplicateOrOversized)
    ));

    let mut bytes = raw_pkg();
    put_u32(&mut bytes, 0x10, 2);
    bytes.truncate(0x1020);
    assert!(matches!(
        Pkg::parse(bytes).unwrap().param_sfo_data(),
        Err(ParamSfoError::ReadEntry {
            source: EntryReadError::InvalidOffset { num: 1 }
        })
    ));
}

#[test]
fn param_sfo_data_limits_the_payload_before_allocation() {
    let mut bytes = raw_pkg();
    put_u32(&mut bytes, 0x1014, 1024 * 1024);
    bytes.resize(0x1020 + 1024 * 1024, 0);
    let pkg = Pkg::parse(bytes).unwrap();
    assert_eq!(pkg.param_sfo_data().unwrap().unwrap().len(), 1024 * 1024);

    let mut bytes = raw_pkg();
    put_u32(&mut bytes, 0x1014, 1024 * 1024 + 1);
    assert!(matches!(
        Pkg::parse(bytes).unwrap().param_sfo_data(),
        Err(ParamSfoError::DuplicateOrOversized)
    ));
}

#[test]
fn param_sfo_data_preserves_payload_and_key_errors() {
    let mut bytes = raw_pkg();
    put_u32(&mut bytes, 0x1010, 0x10ff);
    assert!(matches!(
        Pkg::parse(bytes).unwrap().param_sfo_data(),
        Err(ParamSfoError::ReadData {
            source: EntryDataError::InvalidDataOffset
        })
    ));

    let mut bytes = raw_pkg();
    put_u32(&mut bytes, 0x1008, 0x80000000);
    put_u32(&mut bytes, 0x100c, 0x3000);
    assert!(matches!(
        Pkg::parse(bytes).unwrap().param_sfo_data(),
        Err(ParamSfoError::ReadData {
            source: EntryDataError::NoDecryptionKey { key_index: 3 }
        })
    ));
}

// The directory records 532 logical bytes, while encrypted storage includes
// the entire 544-byte AES block range. No decryption keys are needed to hash it.
fn pkg_with_entry_digest(encrypted: bool) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut bytes = vec![0; 0x1080 + 544];
    put_u32(&mut bytes, 0, 0x7f434e54);
    put_u32(&mut bytes, 0x10, 2);
    put_u32(&mut bytes, 0x18, 0x1000);
    put_u32(&mut bytes, 0x1000, 1); // Digest table.
    put_u32(&mut bytes, 0x1010, 0x1040);
    put_u32(&mut bytes, 0x1014, 64);
    put_u32(&mut bytes, 0x1020, 0x403); // npbind.dat.
    put_u32(&mut bytes, 0x1028, if encrypted { 0x80000000 } else { 0 });
    put_u32(&mut bytes, 0x1030, 0x1080);
    put_u32(&mut bytes, 0x1034, 532);
    bytes[0x1080..].fill(0xa5);
    let stored_size = if encrypted { 544 } else { 532 };
    let digest = Sha256::digest(&bytes[0x1080..0x1080 + stored_size]);
    bytes[0x1060..0x1080].copy_from_slice(&digest);
    bytes
}

#[test]
fn integrity_hashes_encrypted_entry_padding_without_keys() {
    let bytes = pkg_with_entry_digest(true);
    let report = Pkg::parse(bytes.clone())
        .unwrap()
        .verify_integrity(false, || false)
        .unwrap();
    assert_eq!(report.checked, 1);

    let mut corrupt = bytes;
    *corrupt.last_mut().unwrap() ^= 1;
    let error = Pkg::parse(corrupt)
        .unwrap()
        .verify_integrity(false, || false)
        .unwrap_err();
    assert_eq!(error.to_string(), "SHA-256 mismatch: entry 0x00000403");
}

#[test]
fn integrity_does_not_hash_plaintext_entry_alignment_bytes() {
    let mut bytes = pkg_with_entry_digest(false);
    *bytes.last_mut().unwrap() ^= 1;
    let report = Pkg::parse(bytes)
        .unwrap()
        .verify_integrity(false, || false)
        .unwrap();
    assert_eq!(report.checked, 1);
}

#[test]
fn integrity_rejects_truncated_encrypted_padding_even_without_digest() {
    for has_digest_table in [true, false] {
        let mut bytes = pkg_with_entry_digest(true);
        bytes[0x1060..0x1080].fill(0);
        if !has_digest_table {
            // Expose only npbind.dat, omitting the digest-table entry entirely.
            put_u32(&mut bytes, 0x10, 1);
            put_u32(&mut bytes, 0x18, 0x1020);
        }
        bytes.pop();
        let error = Pkg::parse(bytes)
            .unwrap()
            .verify_integrity(false, || false)
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "PKG integrity range outside file");
    }
}

#[test]
fn integrity_checks_encrypted_length_rounding_without_overflow() {
    let mut bytes = pkg_with_entry_digest(true);
    put_u32(&mut bytes, 0x1034, u32::MAX);
    assert!(
        Pkg::parse(bytes)
            .unwrap()
            .verify_integrity(false, || false)
            .is_err()
    );
}
