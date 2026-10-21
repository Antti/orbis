use super::*;
const BS: usize = 4096;
const KEY: [u8; 32] = [0x43; 32];

fn put(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
fn mac(bytes: &[u8]) -> [u8; 32] {
    let mut derive = Hmac::<Sha256>::new_from_slice(&KEY).unwrap();
    derive.update(&[2, 0, 0, 0]);
    derive.update(&[0; 16]);
    let mut hash = Hmac::<Sha256>::new_from_slice(&derive.finalize().into_bytes()).unwrap();
    hash.update(bytes);
    hash.finalize().into_bytes().into()
}
fn pointer(bytes: &mut [u8], at: usize, block: usize) {
    let signature = mac(&bytes[block * BS..(block + 1) * BS]);
    bytes[at..at + 32].copy_from_slice(&signature);
    put(bytes, at + 32, block as u64);
}
fn header_mac(bytes: &mut [u8]) {
    bytes[0x380..0x3a0].fill(0);
    let signature = mac(&bytes[..0x5a0]);
    bytes[0x380..0x3a0].copy_from_slice(&signature);
}

// Independent encoder: one empty root inode, a large inode table, and signed
// S64 pointer blocks. Data inode addressing remains S32.
fn table_image(count: usize) -> Vec<u8> {
    let capacity = BS / 40;
    let extra = if count > 12 + capacity { 3 } else { 1 };
    let mut bytes = vec![0; (1 + count + extra) * BS];
    put(&mut bytes, 0, 1);
    put(&mut bytes, 8, 20130315);
    bytes[0x1c] = 1;
    bytes[0x20..0x24].copy_from_slice(&(BS as u32).to_le_bytes());
    put(&mut bytes, 0x30, 1);
    put(&mut bytes, 0x40, count as u64);
    bytes[BS..BS + 2].copy_from_slice(&0x4000u16.to_le_bytes());
    for i in 0..12 {
        pointer(&mut bytes, 0xb8 + i * 40, i + 1);
    }
    let single = count + 1;
    for i in 0..(count - 12).min(capacity) {
        pointer(&mut bytes, single * BS + i * 40, 13 + i);
    }
    pointer(&mut bytes, 0xb8 + 12 * 40, single);
    if count > 12 + capacity {
        let double = single + 1;
        let leaf = double + 1;
        for i in 0..count - 12 - capacity {
            pointer(&mut bytes, leaf * BS + i * 40, 13 + capacity + i);
        }
        pointer(&mut bytes, double * BS, leaf);
        pointer(&mut bytes, 0xb8 + 13 * 40, double);
    }
    header_mac(&mut bytes);
    bytes
}

#[test]
fn s64_table_signatures_cover_single_and_double_indirect_blocks() {
    for count in [13, 12 + BS / 40 + 1] {
        let bytes = table_image(count);
        let pfs = crate::open_slice(&bytes, None).unwrap();
        let report = pfs.verify_integrity(Some(&KEY), || false).unwrap();
        assert_eq!(report.missing, 0);
        assert_eq!(report.checked, count + if count == 13 { 2 } else { 4 });
        for block in [count, count + 1] {
            let mut damaged = bytes.clone();
            damaged[block * BS + BS - 1] ^= 1;
            let pfs = crate::open_slice(&damaged, None).unwrap();
            let mut failures = Vec::new();
            assert!(
                pfs.verify_integrity_with(
                    Some(&KEY),
                    || false,
                    |check| {
                        if check.status == Status::Failed {
                            failures.push(check.scope.to_owned());
                        }
                        Ok(())
                    }
                )
                .is_err()
            );
            assert_eq!(failures.len(), 1);
            assert!(failures[0].contains(&format!("block {block}")));
        }
    }
}

#[test]
fn table_pointers_must_authenticate_the_blocks_used_by_the_parser() {
    let mut bytes = table_image(13);
    let single = 14 * BS;
    // Keep the MAC valid but redirect the final table pointer to block 12.
    pointer(&mut bytes, single, 12);
    pointer(&mut bytes, 0xb8 + 12 * 40, 14);
    header_mac(&mut bytes);
    let pfs = crate::open_slice(&bytes, None).unwrap();
    assert!(
        pfs.verify_integrity(Some(&KEY), || false)
            .unwrap_err()
            .to_string()
            .contains("different block")
    );
}

#[test]
fn missing_descriptors_keys_and_observer_errors_remain_explicit() {
    let mut bytes = table_image(14);
    bytes[0xb8 + 12 * 40..0xb8 + 13 * 40].fill(0);
    header_mac(&mut bytes);
    let pfs = crate::open_slice(&bytes, None).unwrap();
    let mut missing = Vec::new();
    let report = pfs
        .verify_integrity_with(
            Some(&KEY),
            || false,
            |check| {
                if check.status == Status::Unavailable {
                    missing.push((check.reason, check.count));
                }
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(report.missing, 2);
    assert_eq!(missing, vec![(Some(Reason::MissingDigest), 2)]);
    pfs.verify_integrity_with(
        None,
        || false,
        |check| {
            assert_eq!(check.reason, Some(Reason::MissingKey));
            Ok(())
        },
    )
    .unwrap();
    let error = pfs
        .verify_integrity_with(
            Some(&KEY),
            || false,
            |_| Err(io::ErrorKind::BrokenPipe.into()),
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(
        pfs.verify_integrity(None, || true).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
}
