#![cfg(feature = "cli")]
#[allow(dead_code)]
mod common;
use common::*;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_orbis"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_plans_installs_and_rolls_back_without_touching_base() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = base_game(root);
    fs::write(
        root.join("update.pkg"),
        update("01.01", "base.txt", b"UPDATED"),
    )
    .unwrap();
    let preview = run(
        root,
        &[
            "install",
            "update.pkg",
            "--base",
            "CUSA00000",
            "--dry-run",
            "--json",
        ],
    );
    success(&preview);
    let json: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["plan"]["current_version"], "01.00");
    assert_eq!(json["plan"]["update_version"], "01.01");
    assert!(!root.join(".CUSA00000.orbis-install").exists());
    assert!(!root.join("CUSA00000-UPDATE").exists());

    let installed = run(
        root,
        &["install", "update.pkg", "--base", "CUSA00000", "--quiet"],
    );
    success(&installed);
    assert!(installed.stdout.is_empty());
    assert!(installed.stderr.is_empty());
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    assert_eq!(
        fs::read(root.join("CUSA00000-UPDATE/base.txt")).unwrap(),
        b"UPDATED"
    );

    let again = run(root, &["install", "update.pkg", "--base", "CUSA00000"]);
    assert_eq!(again.status.code(), Some(1));
    assert!(again.stdout.is_empty());
    assert!(String::from_utf8_lossy(&again.stderr).contains("not newer"));
    success(&run(root, &["rollback", "--base", "CUSA00000", "--quiet"]));
    assert!(!root.join("CUSA00000-UPDATE").exists());
    success(&run(root, &["recover", "--base", "CUSA00000"]));
}

#[test]
fn cli_rejects_delta_before_opening_its_filesystem_and_preserves_output() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    base_game(root);
    let mut bytes = update("01.01", "file", b"NEW");
    be32(&mut bytes, 0x74, 0x1e);
    bytes[0x2000..].fill(0);
    fs::write(root.join("delta.pkg"), bytes).unwrap();
    let output = run(
        root,
        &["install", "delta.pkg", "--base", "CUSA00000", "--quiet"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("binary delta"));
    assert!(!root.join(".CUSA00000.orbis-install").exists());
    assert!(!root.join("CUSA00000-UPDATE").exists());
    let info = run(root, &["info", "delta.pkg", "--json"]);
    success(&info);
    let json: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(json["metadata"]["content_type_name"], "DP (Delta Patch)");
}

#[test]
fn cli_usage_and_recovery_errors_are_actionable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    base_game(root);
    for args in [
        vec!["install", "update.pkg"],
        vec!["install", "update.pkg", "--base", "CUSA00000", "--json"],
        vec!["rollback"],
    ] {
        assert_eq!(run(root, &args).status.code(), Some(2));
    }
    let manager = root.join(".CUSA00000.orbis-install");
    fs::create_dir(&manager).unwrap();
    fs::write(manager.join("journal.json"), b"invalid").unwrap();
    fs::write(root.join("update.pkg"), update("01.01", "file", b"NEW")).unwrap();
    let preview = run(
        root,
        &["install", "update.pkg", "--base", "CUSA00000", "--dry-run"],
    );
    assert_eq!(preview.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&preview.stderr).contains("recover --base"));
    assert_eq!(
        run(root, &["recover", "--base", "CUSA00000"]).status.code(),
        Some(1)
    );
    assert!(manager.join("journal.json").exists());
}

#[test]
fn cli_merge_previews_status_and_restores_the_game_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = base_game(root);
    let original_sfo = fs::read(base.join("sce_sys/param.sfo")).unwrap();
    fs::write(
        root.join("update.pkg"),
        update("01.01", "base.txt", b"UPDATED"),
    )
    .unwrap();
    let preview = run(
        root,
        &[
            "install",
            "update.pkg",
            "--base",
            "CUSA00000",
            "--merge",
            "--dry-run",
            "--json",
        ],
    );
    success(&preview);
    let json: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(json["plan"]["mode"], "merge");
    assert!(json["plan"]["backup_bytes"].as_u64().unwrap() > 4);
    assert!(!root.join(".CUSA00000.orbis-install").exists());
    success(&run(
        root,
        &[
            "install",
            "update.pkg",
            "--base",
            "CUSA00000",
            "--merge",
            "--quiet",
        ],
    ));
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"UPDATED");
    let status = run(root, &["status", "--base", "CUSA00000", "--json"]);
    success(&status);
    let json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["mode"], "merge");
    assert_eq!(json["active_version"], "01.01");
    success(&run(root, &["recover", "--base", "CUSA00000"]));
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"UPDATED");
    fs::remove_file(root.join("update.pkg")).unwrap();
    success(&run(root, &["rollback", "--base", "CUSA00000"]));
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    assert_eq!(
        fs::read(base.join("sce_sys/param.sfo")).unwrap(),
        original_sfo
    );
}
