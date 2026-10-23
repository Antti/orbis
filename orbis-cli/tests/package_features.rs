#![cfg(feature = "cli")]
#[allow(dead_code)]
mod common;
use common::*;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_orbis"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}
fn assert_code(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn hash_at(bytes: &mut [u8], slot: usize, start: usize, size: usize) {
    let hash = Sha256::digest(&bytes[start..start + size]);
    bytes[slot..slot + 32].copy_from_slice(&hash);
}

// Independent fixture encoder: seven SC entries, both chain digests, body,
// entry table and ciphertext PFS hashes. All ranges are deliberately explicit.
fn signed_pkg() -> Vec<u8> {
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
    for (i, (_, offset, size)) in entries.iter().enumerate().skip(1) {
        hash_at(&mut b, 0x1e00 + i * 32, *offset as usize, *size as usize);
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

#[test]
fn integrity_checks_all_stored_digests_and_reports_missing_checks() {
    let dir = tempfile::tempdir().unwrap();
    let b = signed_pkg();
    std::fs::write(dir.path().join("good.pkg"), &b).unwrap();
    let output = run(dir.path(), &["verify", "good.pkg", "--integrity"]);
    assert_code(&output, 0);
    assert!(String::from_utf8_lossy(&output.stderr).contains("13 SHA-256"));
    // Remove ancestor hashes to prove that the more specific checks detect
    // corruption independently of the enclosing header/body hash.
    for (offset, label) in [
        (0x1a50, "entry 0x00001000"),
        (0x1e40, "digest table"),
        (0x100, "SC entries 1"),
        (0x120, "SC entries 2"),
        (0x460, "signed PFS region"),
    ] {
        let mut bad = b.clone();
        bad[offset] ^= 1;
        bad[0xfe0..0x1000].fill(0);
        bad[0x160..0x180].fill(0);
        std::fs::write(dir.path().join("bad.pkg"), bad).unwrap();
        let output = run(dir.path(), &["verify", "bad.pkg", "--integrity", "--quiet"]);
        assert_code(&output, 1);
        assert!(String::from_utf8_lossy(&output.stderr).contains(label));
    }
    for (i, offset) in [0x80, 0x1a50, 0x1e40, 0x1f10, b.len() - 1]
        .into_iter()
        .enumerate()
    {
        let mut bad = b.clone();
        bad[offset] ^= 1;
        std::fs::write(dir.path().join("bad.pkg"), bad).unwrap();
        let output = run(dir.path(), &["verify", "bad.pkg", "--integrity", "--quiet"]);
        assert_code(&output, 1);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("SHA-256 mismatch"),
            "case {i}"
        );
    }
    let mut absent = b.clone();
    absent[0x100..0x180].fill(0);
    absent[0xfe0..0x1000].fill(0);
    std::fs::write(dir.path().join("absent.pkg"), absent).unwrap();
    let output = run(
        dir.path(),
        &["verify", "absent.pkg", "--integrity", "--quiet"],
    );
    assert_code(&output, 3);
    assert!(String::from_utf8_lossy(&output.stderr).contains("checks unavailable"));
    assert_code(
        &run(
            dir.path(),
            &["verify", "good.pkg", "--integrity", "--path", "hello.txt"],
        ),
        2,
    );
    // Malformed ranges/table must not panic or turn into "missing checks".
    for (offset, value) in [
        (0x1014, 1),
        (0x1030, u32::MAX),
        (0x1000, 0x10),
        (0x10, u32::MAX),
    ] {
        let mut bad = b.clone();
        be32(&mut bad, offset, value);
        bad[0xfe0..0x1000].fill(0);
        std::fs::write(dir.path().join("bad.pkg"), bad).unwrap();
        assert_code(
            &run(
                dir.path(),
                &["verify", "bad.pkg", "--integrity", "--metadata-only"],
            ),
            1,
        );
    }
    let pkg = orbis_pkg::Pkg::parse(b).unwrap();
    assert_eq!(
        pkg.verify_integrity(true, || true).unwrap_err().kind(),
        std::io::ErrorKind::Interrupted
    );
}

#[test]
fn cli_integrity_checks_signed_outer_pfs_even_when_pkg_digests_are_absent() {
    use hmac::{Hmac, Mac};
    let mut b = update("01.12", "hello.txt", b"hello");
    let image = &mut b[0x2000..];
    // Convert the fixture's unsigned inode table into signed direct pointers.
    let old = image[BS..2 * BS].to_vec();
    image[BS..2 * BS].fill(0);
    image[0x1c] = 1;
    image[0x370..0x380].fill(0x17);
    let mut key = Hmac::<Sha256>::new_from_slice(&[0x43; 32]).unwrap();
    key.update(&2u32.to_le_bytes());
    key.update(&[0x17; 16]);
    let key = key.finalize().into_bytes();
    let sign = |bytes: &mut [u8], at: usize, block: usize| {
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
        mac.update(&bytes[block * BS..(block + 1) * BS]);
        bytes[at..at + 32].copy_from_slice(&mac.finalize().into_bytes());
        le32(bytes, at + 32, block as u32);
    };
    for inode in 0..3 {
        let at = BS + inode * 712;
        image[at..at + 100].copy_from_slice(&old[inode * 168..inode * 168 + 100]);
        let count = u32::from_le_bytes(image[at + 96..at + 100].try_into().unwrap());
        for n in 0..count as usize {
            sign(image, at + 100 + n * 36, 2 + inode + n);
        }
    }
    sign(image, 0xb8, 1);
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
    mac.update(&image[..0x5a0]);
    image[0x380..0x3a0].copy_from_slice(&mac.finalize().into_bytes());
    let mut pkg = orbis_pkg::Pkg::new(b.clone()).unwrap();
    pkg.load_pfs_key().unwrap();
    let report = orbis_cli::PkgExtractor::new(&pkg, orbis_cli::SilentProgress, false)
        .verify_integrity(false)
        .unwrap();
    assert_eq!(report.macs, 9);
    assert!(report.missing > 0);
    let dir = tempfile::tempdir().unwrap();
    let mut missing_key = b.clone();
    be32(&mut missing_key, 0x1020, 0x21); // Remove IMAGE_KEY without changing signed PFS bytes.
    std::fs::write(dir.path().join("missing-key.pkg"), missing_key).unwrap();
    let output = run(
        dir.path(),
        &["verify", "missing-key.pkg", "--integrity", "--json"],
    );
    assert_code(&output, 3);
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        result["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["layer"] == "outer_pfs" && c["reason"] == "missing_key")
    );
    // Corrupt padding in inner image content: reads still work, its outer MAC fails.
    *b.last_mut().unwrap() ^= 1;
    std::fs::write(dir.path().join("game.pkg"), b).unwrap();
    assert_code(&run(dir.path(), &["verify", "game.pkg", "--quiet"]), 0);
    let output = run(
        dir.path(),
        &["verify", "game.pkg", "--integrity", "--quiet"],
    );
    assert_code(&output, 1);
    assert!(String::from_utf8_lossy(&output.stderr).contains("HMAC mismatch"));
}

#[test]
fn richer_info_reports_app_version_and_typed_sfo_without_losing_header_access() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = update("01.12", "hello.txt", b"hello");
    std::fs::write(dir.path().join("game.pkg"), &b).unwrap();
    let output = run(dir.path(), &["info", "game.pkg", "--json"]);
    assert_code(&output, 0);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["metadata"]["content_version"], "00");
    assert_eq!(value["application"]["app_version"], "01.12");
    assert_eq!(
        value["sfo"]["CATEGORY"],
        serde_json::json!({"type":"string", "value":"gp"})
    );
    assert_eq!(value["warnings"], serde_json::json!([]));
    assert!(
        String::from_utf8_lossy(&run(dir.path(), &["info", "game.pkg", "--sfo"]).stdout)
            .contains("APP_VER")
    );
    b[0x1a00] ^= 1;
    std::fs::write(dir.path().join("game.pkg"), &b).unwrap();
    let output = run(dir.path(), &["info", "game.pkg", "--json", "--quiet"]);
    assert_code(&output, 3);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["sfo"].is_null());
    assert_eq!(value["metadata"]["title_id"], "CUSA00000");
    assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
    assert_code(&run(dir.path(), &["info", "game.pkg", "--header-only"]), 0);
    be32(&mut b, 0x10, u32::MAX);
    std::fs::write(dir.path().join("game.pkg"), b).unwrap();
    assert_code(&run(dir.path(), &["info", "game.pkg", "--json"]), 3);
}

#[test]
fn sfo_lookup_keeps_inspection_optional_and_installation_strict() {
    let dir = tempfile::tempdir().unwrap();
    base_game(dir.path());
    let original = update("01.12", "hello.txt", b"hello");
    for defect in ["missing", "duplicate", "oversized"] {
        let mut bytes = original.clone();
        match defect {
            "missing" => be32(&mut bytes, 0x1040, 0x1200),
            "duplicate" => be32(&mut bytes, 0x1020, 0x1000),
            "oversized" => be32(&mut bytes, 0x1054, 1024 * 1024 + 1),
            _ => unreachable!(),
        }
        std::fs::write(dir.path().join("game.pkg"), bytes).unwrap();
        let info = run(dir.path(), &["info", "game.pkg", "--json"]);
        assert_code(&info, if defect == "missing" { 0 } else { 3 });
        let value: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
        assert!(value["application"].is_null());
        assert!(value["sfo"].is_null());
        assert_eq!(value["metadata"]["title_id"], "CUSA00000");
        let message = if defect == "missing" {
            assert_eq!(value["warnings"], serde_json::json!([]));
            "update has no PARAM.SFO"
        } else {
            assert_eq!(
                value["warnings"],
                serde_json::json!([
                    "PARAM.SFO unavailable: duplicate or oversized PARAM.SFO entry; showing header metadata"
                ])
            );
            "duplicate or oversized PARAM.SFO entry"
        };
        let install = run(
            dir.path(),
            &["install", "game.pkg", "--base", "CUSA00000", "--dry-run"],
        );
        assert_code(&install, 1);
        assert!(String::from_utf8_lossy(&install.stderr).contains(message));
        assert!(!dir.path().join(".CUSA00000.orbis-install").exists());
        assert!(!dir.path().join("CUSA00000-UPDATE").exists());
    }
}

#[test]
fn info_keeps_integer_and_binary_types_and_escapes_terminal_text() {
    let fields: Vec<(&str, u16, Vec<u8>)> = vec![
        ("TITLE", 0x204, b"Demo\x1b[31m\0".to_vec()),
        ("APP_VER", 0x204, b"02.03\0".to_vec()),
        ("SYSTEM_VER", 0x404, 0x09030000u32.to_le_bytes().to_vec()),
        (
            "PUBTOOLINFO",
            0x204,
            b"sdk_ver=09030000, c_date=20261020\0".to_vec(),
        ),
        ("BLOB", 4, vec![0, 0xff, 0x80]),
    ];
    let key_start = 20 + fields.len() * 16;
    let mut sfo = vec![0; key_start];
    let mut keys = Vec::new();
    let mut values: Vec<u8> = Vec::new();
    sfo[..4].copy_from_slice(b"\0PSF");
    le32(&mut sfo, 4, 0x101);
    le32(&mut sfo, 8, key_start as u32);
    le32(&mut sfo, 16, fields.len() as u32);
    for (i, (key, format, value)) in fields.iter().enumerate() {
        let at = 20 + i * 16;
        sfo[at..at + 2].copy_from_slice(&(keys.len() as u16).to_le_bytes());
        sfo[at + 2..at + 4].copy_from_slice(&format.to_le_bytes());
        le32(&mut sfo, at + 4, value.len() as u32);
        le32(&mut sfo, at + 8, value.len() as u32);
        le32(&mut sfo, at + 12, values.len() as u32);
        keys.extend(key.bytes().chain([0]));
        values.extend(value);
    }
    le32(&mut sfo, 12, (key_start + keys.len()) as u32);
    sfo.extend(keys);
    sfo.extend(values);
    let mut b = update("01.12", "hello.txt", b"hello");
    b[0x1a00..0x1a00 + sfo.len()].copy_from_slice(&sfo);
    be32(&mut b, 0x1054, sfo.len() as u32);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("game.pkg"), b).unwrap();
    let output = run(dir.path(), &["info", "game.pkg", "--json"]);
    assert_code(&output, 0);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["application"],
        serde_json::json!({
            "app_version": "02.03",
            "build": {"c_date": "20261020", "sdk_ver": "09030000"},
            "category": null,
            "content_id": null,
            "package_version": null,
            "region_from_content_id": "North America",
            "required_system_version": "9.03",
            "title": "Demo\u{1b}[31m",
            "title_id": null,
        })
    );
    assert_eq!(
        value["sfo"]["SYSTEM_VER"],
        serde_json::json!({"type": "integer", "value": 0x09030000u32})
    );
    assert_eq!(
        value["sfo"]["BLOB"],
        serde_json::json!({"type": "binary", "hex": "00ff80"})
    );
    // Preserve the existing sorted JSON order, including binary field keys.
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap())
    );
    let output = run(dir.path(), &["info", "game.pkg", "--sfo"]);
    assert_code(&output, 0);
    assert!(!output.stdout.contains(&0x1b));
    let text = String::from_utf8(output.stdout).unwrap();
    let (_, application) = text.split_once("\nApplication:\n").unwrap();
    assert_eq!(
        application,
        concat!(
            "  app_version: 02.03\n",
            "  build: {\"c_date\":\"20261020\",\"sdk_ver\":\"09030000\"}\n",
            "  region_from_content_id: North America\n",
            "  required_system_version: 9.03\n",
            "  title: Demo\\u{1b}[31m\n",
            "\nPARAM.SFO:\n",
            "  APP_VER: {\"type\":\"string\",\"value\":\"02.03\"}\n",
            "  BLOB: {\"hex\":\"00ff80\",\"type\":\"binary\"}\n",
            "  PUBTOOLINFO: {\"type\":\"string\",\"value\":\"sdk_ver=09030000, c_date=20261020\"}\n",
            "  SYSTEM_VER: {\"type\":\"integer\",\"value\":151191552}\n",
            "  TITLE: {\"type\":\"string\",\"value\":\"Demo\\u001b[31m\"}\n",
        )
    );
}

fn named_pkg(name: &[u8]) -> Vec<u8> {
    let mut b = update("01.12", "hello.txt", b"hello");
    be32(&mut b, 0x10, 5);
    for (i, id, offset, size) in [
        (3, 0x200, 0x1e00, name.len() as u32 + 2),
        (4, 0xdeadbeef, 0x1f00, 3),
    ] {
        let at = 0x1000 + i * 32;
        be32(&mut b, at, id);
        be32(&mut b, at + 16, offset);
        be32(&mut b, at + 20, size);
    }
    be32(&mut b, 0x1084, 1);
    b[0x1e01..0x1e01 + name.len()].copy_from_slice(name);
    b[0x1f00..0x1f03].copy_from_slice(b"EXT");
    b
}

#[test]
fn named_unknown_entries_extract_and_unsafe_names_fail_before_writes() {
    let dir = tempfile::tempdir().unwrap();
    // named_pkg writes a 3-byte payload.
    let mut b = named_pkg(b"extra/details.bin");
    std::fs::write(dir.path().join("game.pkg"), &b).unwrap();
    let output = run(
        dir.path(),
        &[
            "extract",
            "game.pkg",
            "--metadata-only",
            "--path",
            "sce_sys/extra",
            "-o",
            "out",
        ],
    );
    assert_code(&output, 0);
    assert_eq!(
        std::fs::read(dir.path().join("out/sce_sys/extra/details.bin")).unwrap(),
        b"EXT"
    );
    for name in [
        "../escape",
        "/absolute",
        "C:/escape",
        "a\\b",
        "NUL",
        "COM1.txt",
        "bad.",
        "a//b",
        "a/./b",
        "a:stream",
    ] {
        std::fs::write(dir.path().join("bad.pkg"), named_pkg(name.as_bytes())).unwrap();
        assert_code(
            &run(
                dir.path(),
                &["extract", "bad.pkg", "--metadata-only", "-o", "bad-out"],
            ),
            1,
        );
        assert!(!dir.path().join("bad-out").exists());
    }
    // An offset into the middle of a name must not silently rename an entry.
    be32(&mut b, 0x1084, 2);
    std::fs::write(dir.path().join("bad.pkg"), b).unwrap();
    assert_code(&run(dir.path(), &["list", "bad.pkg", "--entries"]), 1);
    std::fs::write(dir.path().join("bad.pkg"), named_pkg(b"PARAM.SFO")).unwrap();
    assert_code(
        &run(
            dir.path(),
            &["extract", "bad.pkg", "--metadata-only", "-o", "bad-out"],
        ),
        1,
    );
    assert!(!dir.path().join("bad-out").exists());
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
const PASSCODE: &[u8] = b"0123456789abcdef0123456789abcdef";

fn passcoded_pkg(slot: usize) -> Vec<u8> {
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

#[test]
fn passcodes_unlock_every_entry_slot_and_wrong_passcodes_never_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    for slot in 0..7 {
        let bytes = passcoded_pkg(slot);
        let mut pkg = orbis_pkg::Pkg::parse(bytes.clone()).unwrap();
        pkg.load_passcode(PASSCODE).unwrap();
        let (entry, _) = pkg.find_entry(orbis_pkg::entry::EntryId::ParamSfo).unwrap();
        let data = pkg.entry_data(&entry).unwrap();
        assert_eq!(
            orbis_pkg::sfo::Sfo::parse(&data)
                .unwrap()
                .string("APP_VER")
                .unwrap(),
            "01.12"
        );
        assert!(
            pkg.load_passcode(b"00000000000000000000000000000000")
                .is_err()
        );
        assert_eq!(pkg.entry_data(&entry).unwrap(), data); // failed load leaves valid keys intact
        if slot == 2 {
            std::fs::write(dir.path().join("game.pkg"), bytes).unwrap();
        }
    }
    std::fs::write(dir.path().join("passcode"), [PASSCODE, b"\r\n"].concat()).unwrap();
    assert_code(
        &run(
            dir.path(),
            &["info", "game.pkg", "--json", "--passcode-file", "passcode"],
        ),
        0,
    );
    assert_code(
        &run(
            dir.path(),
            &[
                "extract",
                "game.pkg",
                "--metadata-only",
                "--passcode-file",
                "passcode",
                "-o",
                "out",
            ],
        ),
        0,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_orbis"))
        .current_dir(dir.path())
        .args([
            "verify",
            "game.pkg",
            "--metadata-only",
            "--passcode-file",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(PASSCODE).unwrap();
    assert_code(&child.wait_with_output().unwrap(), 0);
    let base = base_game(dir.path());
    assert_code(
        &run(
            dir.path(),
            &[
                "install",
                "game.pkg",
                "--base",
                base.to_str().unwrap(),
                "--passcode-file",
                "passcode",
            ],
        ),
        0,
    );
    let installed = std::fs::read(dir.path().join("CUSA00000-UPDATE/sce_sys/param.sfo")).unwrap();
    assert_eq!(
        orbis_pkg::sfo::Sfo::parse(&installed)
            .unwrap()
            .string("APP_VER")
            .unwrap(),
        "01.12"
    );
    for offset in [0x1100, 0x1120, 0x1120 + 6 * 32] {
        let mut bad = passcoded_pkg(2);
        bad[offset] ^= 1;
        let mut pkg = orbis_pkg::Pkg::parse(bad).unwrap();
        assert!(pkg.load_passcode(PASSCODE).is_err());
    }
    for secret in [
        b"bad".as_slice(),
        b"00000000000000000000000000000000",
        &[b'x'; 36],
    ] {
        std::fs::write(dir.path().join("passcode"), secret).unwrap();
        let output = run(
            dir.path(),
            &[
                "extract",
                "game.pkg",
                "--passcode-file",
                "passcode",
                "-o",
                "never",
            ],
        );
        assert_code(&output, 1);
        assert!(!dir.path().join("never").exists());
        assert!(
            !String::from_utf8_lossy(&output.stderr)
                .contains(&String::from_utf8_lossy(secret).to_string())
        );
    }
}

#[test]
fn integrity_json_reports_success_omissions_and_failures() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = signed_pkg();
    std::fs::write(dir.path().join("good.pkg"), &bytes).unwrap();
    let check = |name: &str, expected| {
        let output = run(
            dir.path(),
            &["verify", name, "--integrity", "--json", "--quiet"],
        );
        assert_code(&output, expected);
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let result = check("good.pkg", 0);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["status"], "complete");
    assert_eq!(result["summary"]["digests"], 13);
    assert_eq!(result["summary"]["failed"], 0);
    assert_eq!(result["readability"]["status"], "passed");
    let checks = result["checks"].as_array().unwrap();
    assert_eq!(
        checks
            .iter()
            .filter(|c| c["reason"] == "unsigned_image")
            .count(),
        2
    );
    assert!(
        checks
            .iter()
            .filter(|c| c["layer"] == "pkg")
            .all(|c| c["status"] == "passed")
    );

    let mut absent = bytes.clone();
    absent[0xfe0..0x1000].fill(0);
    std::fs::write(dir.path().join("absent.pkg"), &absent).unwrap();
    let result = check("absent.pkg", 3);
    assert_eq!(result["summary"]["unavailable"], 1);
    assert_eq!(result["checks"][0]["reason"], "missing_digest");
    assert_eq!(result["status"], "incomplete");

    let mut bad = bytes;
    bad[0x80] ^= 1;
    std::fs::write(dir.path().join("bad.pkg"), &bad).unwrap();
    let result = check("bad.pkg", 1);
    assert_eq!(result["status"], "failed");
    assert_eq!(result["summary"]["failed"], 1);
    assert_eq!(result["checks"][0]["scope"], "PKG header");
    assert_eq!(result["checks"][0]["reason"], "digest_mismatch");
    assert!(result["readability"].is_null());
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("SHA-256 mismatch")
    );

    // Input and structural errors still produce complete machine-readable output.
    for name in ["missing.pkg", "truncated.pkg", "invalid-range.pkg"] {
        if name == "truncated.pkg" {
            std::fs::write(dir.path().join(name), b"short").unwrap();
        }
        if name == "invalid-range.pkg" {
            be64(&mut absent, 0x28, u64::MAX);
            std::fs::write(dir.path().join(name), &absent).unwrap();
        }
        let result = check(name, 1);
        assert_eq!(result["status"], "failed");
        assert!(result["error"].is_string());
    }
    let output = run(
        dir.path(),
        &[
            "verify",
            "good.pkg",
            "--integrity",
            "--json",
            "--metadata-only",
        ],
    );
    assert_code(&output, 0);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["metadata_only"], true);
    assert!(
        value["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["layer"] == "pkg")
    );
    assert_code(&run(dir.path(), &["verify", "good.pkg", "--json"]), 2);
}

#[test]
fn pkg_integrity_observer_can_stop_without_finishing_the_scan() {
    let bytes = signed_pkg();
    let pkg = orbis_pkg::Pkg::parse(bytes).unwrap();
    let mut calls = 0;
    let error = pkg
        .verify_integrity_with(
            true,
            || false,
            |_| {
                calls += 1;
                Err(std::io::ErrorKind::BrokenPipe.into())
            },
        )
        .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}
