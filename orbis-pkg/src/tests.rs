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
