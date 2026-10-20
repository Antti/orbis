#![cfg(feature = "cli")]

use std::process::Command;

#[test]
fn documented_commands_have_help() {
    for command in ["extract", "info", "list", "verify", "completions"] {
        let output = Command::new(env!("CARGO_BIN_EXE_orbis-pkg-util"))
            .args([command, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{command}: {:?}", output.stderr);
    }
}

#[test]
fn reports_the_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_orbis-pkg-util"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains(env!("CARGO_PKG_VERSION"))
    );
}

mod common;
use common::*;
use std::path::Path;
use std::process::Output;

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_orbis-pkg-util"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
fn sample(dir: &Path) {
    std::fs::write(dir.join("game.pkg"), pkg(pfs("hello.txt", b"hello", false))).unwrap();
}

#[test]
fn inspection_and_json_work_without_keys_or_a_readable_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = pkg(pfs("hello.txt", b"hello", false));
    bytes[0x1900..0x1a00].fill(0);
    bytes[0x2000..0x2008].fill(0);
    std::fs::write(dir.path().join("game.pkg"), bytes).unwrap();
    for args in [
        vec!["info", "game.pkg", "--json"],
        vec!["list", "game.pkg", "--entries", "--json"],
    ] {
        let output = run(dir.path(), &args);
        assert!(output.status.success(), "{}", text(&output.stderr));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert!(output.stderr.is_empty());
    }
    let output = run(dir.path(), &["list", "game.pkg", "--json"]);
    assert_eq!(output.status.code(), Some(3));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["items"].as_array().unwrap().len(), 3);
    assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
    assert!(text(&output.stderr).contains("filesystem unavailable"));
    let output = run(
        dir.path(),
        &[
            "extract",
            "game.pkg",
            "--metadata-only",
            "--path",
            "sce_sys/param.sfo",
            "-o",
            "metadata",
        ],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        std::fs::read(dir.path().join("metadata/sce_sys/param.sfo")).unwrap(),
        b"NEW"
    );
}

#[test]
fn listing_matches_extraction_paths_and_has_stable_json_fields() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let output = run(dir.path(), &["list", "game.pkg", "--json"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["warnings"], serde_json::json!([]));
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(
        items[0],
        serde_json::json!({
            "path": "hello.txt", "source": "filesystem", "kind": "file", "size": 5,
            "entry_id": null, "entry_encrypted": null, "key_index": null,
        })
    );
    assert!(
        items
            .iter()
            .any(|item| item["path"] == "sce_sys/param.sfo" && item["source"] == "metadata")
    );
    let output = run(dir.path(), &["list", "game.pkg", "--files"]);
    assert!(text(&output.stdout).contains("hello.txt"));
    assert!(!text(&output.stdout).contains("param.sfo"));
    let output = run(dir.path(), &["info", "game.pkg", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["metadata"]["title_id"], "CUSA00000");
    assert_eq!(value["metadata"]["content_version"], "00");
    assert!(value["metadata"]["pkg_size"].is_u64());
}

#[test]
fn extraction_reports_success_on_stderr_and_quiet_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let output = run(dir.path(), &["extract", "game.pkg"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(output.stdout.is_empty());
    assert!(text(&output.stderr).contains("Extracted 4 files"));
    assert!(text(&output.stderr).contains("Extracting filesystem"));
    assert_eq!(
        std::fs::read(dir.path().join("CUSA00000/hello.txt")).unwrap(),
        b"hello"
    );
    let output = run(
        dir.path(),
        &["--quiet", "extract", "game.pkg", "-o", "quiet"],
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
}

#[test]
fn quiet_extraction_still_warns_and_returns_incomplete_for_missing_keys() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = pkg(pfs("hello.txt", b"hello", false));
    be32(&mut bytes, 0x1048, 0x80000000);
    be32(&mut bytes, 0x104c, 0x1000);
    std::fs::write(dir.path().join("game.pkg"), bytes).unwrap();
    for quiet in [false, true] {
        let mut args = vec![
            "extract",
            "game.pkg",
            "-o",
            if quiet { "quiet" } else { "normal" },
        ];
        if quiet {
            args.push("--quiet");
        }
        let output = run(dir.path(), &args);
        assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
        let stderr = text(&output.stderr);
        assert!(stderr.contains("sce_sys/param.sfo"));
        assert!(stderr.contains("slot 1"));
        assert!(stderr.contains("Incomplete"));
        assert!(!stderr.contains("Extracted 4") && !stderr.contains("Done"));
        assert!(output.stdout.is_empty());
    }
    let output = run(dir.path(), &["verify", "game.pkg", "-q"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(text(&output.stderr).contains("slot 1"));
}

#[test]
fn unknown_entries_are_visible_and_not_silently_lost() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = pkg(pfs("hello.txt", b"hello", false));
    be32(&mut bytes, 0x1040, 0xdeadbeef);
    std::fs::write(dir.path().join("game.pkg"), bytes).unwrap();
    let output = run(dir.path(), &["extract", "game.pkg", "-o", "out", "-q"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(text(&output.stderr).contains("unknown entry ID"));
    let output = run(dir.path(), &["list", "game.pkg", "--entries", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["entry_id"] == 0xdeadbeefu32 && item["path"].is_null())
    );
    // An unknown filename does not prevent checking the raw entry payload.
    assert!(
        run(dir.path(), &["verify", "game.pkg", "-q"])
            .status
            .success()
    );
}

#[test]
fn selected_paths_do_not_extract_other_files_and_missing_paths_fail_early() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let output = run(
        dir.path(),
        &[
            "extract",
            "game.pkg",
            "-o",
            "selected",
            "--path",
            "hello.txt",
            "--path",
            "sce_sys/param.sfo",
        ],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(dir.path().join("selected/hello.txt").is_file());
    assert!(dir.path().join("selected/sce_sys/param.sfo").is_file());
    assert!(!dir.path().join("selected/sce_sys/entry_keys").exists());
    let output = run(
        dir.path(),
        &["extract", "game.pkg", "-o", "missing", "--path", "absent"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("was not found"));
    assert!(!dir.path().join("missing").exists());
}

#[test]
fn verify_reads_payloads_and_never_creates_extracted_files() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let output = run(dir.path(), &["verify", "game.pkg"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(output.stdout.is_empty());
    assert!(text(&output.stderr).contains("Checked 4 files"));
    assert!(text(&output.stderr).contains("authenticity was not checked"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    let mut data = pfsc(BS as u32, BS as u64, BS as u64, &[80, 83]).0;
    data.truncate(83);
    data[80..].copy_from_slice(b"bad");
    std::fs::write(
        dir.path().join("bad.pkg"),
        pkg(pfs("hello.txt", &data, true)),
    )
    .unwrap();
    let output = run(dir.path(), &["verify", "bad.pkg", "-q"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("decompression"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn failed_forced_extraction_preserves_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = pfsc(BS as u32, BS as u64, BS as u64, &[80, 83]).0;
    data.truncate(83);
    data[80..].copy_from_slice(b"bad");
    std::fs::write(
        dir.path().join("game.pkg"),
        pkg(pfs("hello.txt", &data, true)),
    )
    .unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("hello.txt"), b"KEEP").unwrap();
    let output = run(
        dir.path(),
        &[
            "extract",
            "game.pkg",
            "-o",
            "out",
            "--force",
            "--path",
            "hello.txt",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(std::fs::read(out.join("hello.txt")).unwrap(), b"KEEP");
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1);
}

#[test]
fn errors_are_actionable_and_usage_errors_have_distinct_status() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let output = run(dir.path(), &["info", "."]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("directory or special file"));
    assert!(!text(&output.stderr).contains("memory map"));
    assert_eq!(run(dir.path(), &["extract"]).status.code(), Some(2));
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("hello.txt"), b"KEEP").unwrap();
    let output = run(dir.path(), &["extract", "game.pkg", "-o", "out"]);
    assert_eq!(output.status.code(), Some(1));
    let error = text(&output.stderr);
    assert!(error.contains("--output") && error.contains("--force"));
    assert!(!error.contains("SNAFU_RAW_ERROR_MESSAGES"));
    assert!(!out.join("sce_sys").exists());
}

#[test]
fn completion_scripts_include_commands_and_options() {
    let dir = tempfile::tempdir().unwrap();
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = run(dir.path(), &["completions", shell]);
        assert!(output.status.success(), "{shell}: {}", text(&output.stderr));
        let script = text(&output.stdout);
        assert!(
            script.contains("verify") && script.contains("metadata-only"),
            "{shell}"
        );
        assert!(output.stderr.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn closed_stdout_does_not_panic() {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};
    use std::process::Stdio;
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    for args in [
        vec!["info", "game.pkg"],
        vec!["list", "game.pkg", "--json"],
        vec!["completions", "bash"],
    ] {
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(reader);
        let output = Command::new(env!("CARGO_BIN_EXE_orbis-pkg-util"))
            .current_dir(dir.path())
            .args(args)
            .stdout(Stdio::from(OwnedFd::from(writer)))
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn encrypted_metadata_and_filesystem_work_with_deferred_key_loading() {
    use aes::{
        Aes128,
        cipher::{BlockEncryptMut, KeyInit, KeyIvInit},
    };
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = pkg(pfs("hello.txt", b"hello", false));
    // Encrypt both IMAGE_KEY and param.sfo using an independently encoded
    // AES-CBC entry key derived from this fixture's known slot-3 secret.
    for entry in [0x1020, 0x1040] {
        be32(&mut bytes, entry + 8, 0x80000000);
        be32(&mut bytes, entry + 12, 0x3000);
        let mut hash = Sha256::new();
        hash.update(&bytes[entry..entry + 32]);
        hash.update([0x42; 32]);
        let secret = hash.finalize();
        let mut cipher =
            cbc::Encryptor::<Aes128>::new_from_slices(&secret[16..], &secret[..16]).unwrap();
        let offset = u32::from_be_bytes(bytes[entry + 16..entry + 20].try_into().unwrap()) as usize;
        let size = u32::from_be_bytes(bytes[entry + 20..entry + 24].try_into().unwrap()) as usize;
        for block in bytes[offset..offset + size.div_ceil(16) * 16]
            .as_chunks_mut::<16>()
            .0
        {
            cipher.encrypt_block_mut(block.into());
        }
    }
    let outer = &mut bytes[0x2000..];
    outer[0x1c] |= 4;
    let seed = [0x17; 16];
    outer[0x370..0x380].copy_from_slice(&seed);
    let mut hmac = <Hmac<Sha256> as Mac>::new_from_slice(&[0x43; 32]).unwrap();
    hmac.update(&[1, 0, 0, 0]);
    hmac.update(&seed);
    let keys = hmac.finalize().into_bytes();
    let cipher = xts_mode::Xts128::<Aes128>::new(
        Aes128::new_from_slice(&keys[16..]).unwrap(),
        Aes128::new_from_slice(&keys[..16]).unwrap(),
    );
    for (index, block) in outer.as_chunks_mut::<BS>().0.iter_mut().enumerate().skip(1) {
        cipher.encrypt_sector(block, xts_mode::get_tweak_default(index as u128));
    }
    std::fs::write(dir.path().join("game.pkg"), &bytes).unwrap();
    for args in [
        vec!["list", "game.pkg", "--files"],
        vec!["verify", "game.pkg", "-q"],
        vec!["extract", "game.pkg", "-o", "out", "-q"],
    ] {
        let output = run(dir.path(), &args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            text(&output.stderr)
        );
    }
    assert_eq!(
        std::fs::read(dir.path().join("out/hello.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read(dir.path().join("out/sce_sys/param.sfo")).unwrap(),
        b"NEW"
    );
    // Break only the PFS key. Metadata remains readable, including encrypted
    // entries when slot 3 can still be loaded independently.
    bytes[0x1900..0x1a00].fill(0);
    std::fs::write(dir.path().join("bad-key.pkg"), &bytes).unwrap();
    for args in [
        vec!["info", "bad-key.pkg", "--json"],
        vec!["list", "bad-key.pkg", "--entries"],
        vec![
            "extract",
            "bad-key.pkg",
            "-o",
            "metadata",
            "--metadata-only",
            "--path",
            "sce_sys/param.sfo",
        ],
    ] {
        let output = run(dir.path(), &args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            text(&output.stderr)
        );
    }
    let output = run(dir.path(), &["list", "bad-key.pkg", "--json"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_ok());
    assert!(text(&output.stderr).contains("decrypt EKPFS"));
}

// APFS rejects non-UTF-8 filenames; Linux filesystems allow this input.
#[cfg(target_os = "linux")]
#[test]
fn json_handles_non_utf8_input_paths_without_panicking() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let name = std::ffi::OsString::from_vec(b"game-\xff.pkg".to_vec());
    std::fs::write(
        dir.path().join(&name),
        pkg(pfs("hello.txt", b"hello", false)),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_orbis-pkg-util"))
        .current_dir(dir.path())
        .arg("info")
        .arg(&name)
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_ok());
}
