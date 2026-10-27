//! Fixed external vectors catch backend differences even when a synthetic
//! package's encoder and decoder happen to use the same crypto backend.
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

fn hex<const N: usize>(value: &str) -> [u8; N] {
    assert_eq!(value.len(), N * 2);
    std::array::from_fn(|i| u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).unwrap())
}

#[test]
fn aes128_matches_fips197_vector() {
    // FIPS 197, Appendix C.1 (2001).
    let key = hex::<16>("000102030405060708090a0b0c0d0e0f");
    let plain = hex::<16>("00112233445566778899aabbccddeeff");
    let encrypted = hex::<16>("69c4e0d86a7b0430d8cdb78070b4c55a");
    let cipher = aes::Aes128::new(&key.into());
    // Multiple blocks also exercise the backend's parallel block path.
    let mut blocks = [plain.into(); 17];
    cipher.encrypt_blocks(&mut blocks);
    assert!(blocks.iter().all(|b| b.as_slice() == encrypted));
    cipher.decrypt_blocks(&mut blocks);
    assert!(blocks.iter().all(|b| b.as_slice() == plain));
}

#[test]
fn sha256_matches_nist_long_message() {
    // NIST SHA-256 example: one million ASCII 'a' bytes.
    let input = vec![b'a'; 1_000_000];
    let expected = hex::<32>("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    for chunk_size in [63, 64, 65, 4096] {
        let mut hash = Sha256::new();
        for chunk in input.chunks(chunk_size) {
            hash.update(chunk);
        }
        assert_eq!(hash.finalize().as_slice(), expected);
    }
}

#[test]
fn hmac_sha256_matches_rfc4231_vector() {
    // RFC 4231, test case 1.
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&[0x0b; 20]).unwrap();
    mac.update(b"Hi There");
    let expected = hex::<32>("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    assert_eq!(mac.finalize().into_bytes().as_slice(), expected);
}
