use super::common::*;
use sha2::{Digest, Sha256};

fn hash_at(bytes: &mut [u8], slot: usize, start: usize, size: usize) {
    let hash = Sha256::digest(&bytes[start..start + size]);
    bytes[slot..slot + 32].copy_from_slice(&hash);
}

// Independent fixture encoder: seven SC entries, both chain digests, body,
// entry table and ciphertext PFS hashes. All ranges are deliberately explicit.
pub fn signed_pkg() -> Vec<u8> {
    signed_pkg_with_metadata(false)
}

pub fn signed_pkg_with_metadata(locked: bool) -> Vec<u8> {
    let mut b = update("01.12", "hello.txt", b"hello");
    let sfo_size = u32::from_be_bytes(b[0x1054..0x1058].try_into().unwrap());
    let entries = [
        (1, 0x1e00, 224),
        (0x10, 0x1100, 2048),
        (0x20, 0x1900, 256),
        (0x80, 0x1f00, 32),
        (0x100, 0x1000, 224),
        (0x200, 0x1f20, 1),
        (0x1000, 0x1a00, sfo_size),
    ];
    be32(&mut b, 0x10, 7);
    b[0x14..0x16].copy_from_slice(&6u16.to_be_bytes());
    be64(&mut b, 0x20, 0x1000);
    be64(&mut b, 0x28, 0x1000);
    be32(&mut b, 0x438, BS as u32);
    for (i, (id, offset, size)) in entries.iter().enumerate() {
        let at = 0x1000 + i * 32;
        b[at..at + 32].fill(0);
        be32(&mut b, at, *id);
        be32(&mut b, at + 16, *offset);
        be32(&mut b, at + 20, *size);
    }
    if locked {
        // Stored bytes can be hashed without the missing decryption key. The
        // readability pass must skip this entry instead of claiming completion.
        be32(&mut b, 0x10c8, 0x80000000);
        be32(&mut b, 0x10cc, 2 << 12);
    }
    for (i, (_, offset, size)) in entries.iter().enumerate().skip(1) {
        let size = if locked && i == 6 {
            size.div_ceil(16) * 16
        } else {
            *size
        };
        hash_at(&mut b, 0x1e00 + i * 32, *offset as usize, size as usize);
    }
    for (slot, indices) in [(0x100, vec![1, 2, 3, 4, 0]), (0x120, vec![1, 2, 3, 4])] {
        let mut chain = Vec::new();
        for i in indices {
            let (_, offset, size) = entries[i];
            let size = if slot == 0x120 && i == 4 { 192 } else { size };
            chain.extend_from_slice(&b[offset as usize..(offset + size) as usize]);
        }
        b[slot..slot + 32].copy_from_slice(&Sha256::digest(chain));
    }
    hash_at(&mut b, 0x140, 0x1e00, 224);
    hash_at(&mut b, 0x160, 0x1000, 0x1000);
    let pfs_len = b.len() - 0x2000;
    hash_at(&mut b, 0x440, 0x2000, pfs_len);
    hash_at(&mut b, 0x460, 0x2000, BS);
    hash_at(&mut b, 0xfe0, 0, 0xfe0);
    b
}

// Fixed vectors calculated independently with Python hashlib for this content
// ID and passcode, rather than the production derivation.
const KEYS: [&str; 7] = [
    "c9c4a03f3d358b8f7db7152e2c37b382be962ebaf230ae68419f46f31ce31d48",
    "5db3b4a5b9572ff5dd9361d60a301235469170e6eec42c411d8cb2c349d43fe4",
    "d9f8cb0e2ce5ad4999671515d6ffb16248b3182db01f0d0bded42db6e9e3efca",
    "52963782f80bef23551a2aa0a8733bcd12a0a84b29d128f6697ab149c14dd48d",
    "fe9d895cf88071f06864ce8c38bd99fa603656a0bfaaf9e551d57b76ca0c0d0c",
    "9aed4edb758be5c9a6edd4c0838405465a3f17683c17191ac2d414151c0e8391",
    "58fe27ac6c37d7edfcdaaba07d5414bbf39fd45f36a6360ae234a76d9425568b",
];
pub const PASSCODE: &[u8] = b"0123456789abcdef0123456789abcdef";

pub fn passcoded_pkg(slot: usize) -> Vec<u8> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    let mut b = update("01.12", "hello.txt", b"hello");
    let mut cid = [0; 48];
    cid[..36].copy_from_slice(&b[0x40..0x64]);
    b[0x1100..0x1120].copy_from_slice(&Sha256::digest(cid));
    let keys: Vec<Vec<u8>> = KEYS
        .iter()
        .map(|s| {
            (0..64)
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        })
        .collect();
    for (i, key) in keys.iter().enumerate() {
        let digest = Sha256::digest(key);
        for j in 0..32 {
            b[0x1120 + i * 32 + j] = digest[j] ^ key[j];
        }
    }
    be32(&mut b, 0x1048, 0x80000000);
    be32(&mut b, 0x104c, (slot as u32) << 12);
    let size = u32::from_be_bytes(b[0x1054..0x1058].try_into().unwrap()) as usize;
    let key = Sha256::digest([&b[0x1040..0x1060], &keys[slot]].concat());
    let mut cipher =
        cbc::Encryptor::<aes::Aes128>::new_from_slices(&key[16..], &key[..16]).unwrap();
    for block in b[0x1a00..0x1a00 + size.div_ceil(16) * 16]
        .as_chunks_mut::<16>()
        .0
    {
        cipher.encrypt_block_mut(block.into());
    }
    b
}
