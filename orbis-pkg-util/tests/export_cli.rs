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
    Command::new(env!("CARGO_BIN_EXE_orbis-pkg-util"))
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
    assert!(output.stdout.is_empty());
}

#[test]
fn exports_installed_version_and_reports_destination_with_quiet_support() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let base = base_game(root);
    fs::write(base.join("unchanged"), b"BASE ASSET").unwrap();
    fs::write(
        root.join("update.pkg"),
        update("01.09", "base.txt", b"UPDATED"),
    )
    .unwrap();
    success(&run(
        root,
        &["install", "update.pkg", "--base", "CUSA00000", "-q"],
    ));
    for quiet in [false, true] {
        let destination = if quiet { "quiet" } else { "merged" };
        let mut args = vec!["export", "--base", "CUSA00000", "-o", destination];
        if quiet {
            args.push("-q");
        }
        let output = run(root, &args);
        success(&output);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if quiet {
            assert!(stderr.is_empty());
        } else {
            assert!(
                stderr.contains("Exported CUSA00000 version 01.09"),
                "{stderr}"
            );
            assert!(stderr.contains("merged"));
        }
        assert_eq!(
            fs::read(root.join(destination).join("base.txt")).unwrap(),
            b"UPDATED"
        );
        assert_eq!(
            fs::read(root.join(destination).join("unchanged")).unwrap(),
            b"BASE ASSET"
        );
        assert!(!root.join(destination).join(".orbis-install.json").exists());
    }
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    success(&run(root, &["rollback", "--base", "CUSA00000", "-q"]));
    assert_eq!(fs::read(root.join("merged/base.txt")).unwrap(), b"UPDATED");
}

#[test]
fn export_usage_and_destination_errors_explain_the_required_action() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    base_game(root);
    for args in [
        vec!["export"],
        vec!["export", "--base", "CUSA00000"],
        vec!["export", "-o", "merged"],
    ] {
        assert_eq!(run(root, &args).status.code(), Some(2));
    }
    fs::create_dir(root.join("existing")).unwrap();
    let output = run(
        root,
        &["export", "--base", "CUSA00000", "-o", "existing", "-q"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("choose a new --output"));
    assert!(!root.join(".CUSA00000.orbis-install").exists());
    let completion = run(root, &["completions", "fish"]);
    assert!(completion.status.success());
    assert!(String::from_utf8_lossy(&completion.stdout).contains("export"));
}

#[test]
fn dry_run_reports_overlay_and_space_without_creating_state_or_output() {
    let temp = tempfile::tempdir().unwrap();
    base_game(temp.path());
    let output = run(
        temp.path(),
        &[
            "export",
            "--base",
            "CUSA00000",
            "-o",
            "merged",
            "--dry-run",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["schema_version"], 1);
    assert_eq!(plan["base_version"], "01.00");
    assert!(plan["update_version"].is_null());
    assert!(plan["files"].as_u64().unwrap() >= 2);
    assert!(
        plan["filesystem"]["required_free_bytes"].as_u64().unwrap()
            >= 2 * plan["bytes"].as_u64().unwrap()
    );
    assert!(!temp.path().join("merged").exists());
    assert!(!temp.path().join(".CUSA00000.orbis-install").exists());
}

#[test]
fn verifies_export_and_reports_changed_missing_and_unexpected_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    base_game(root);
    success(&run(
        root,
        &["export", "--base", "CUSA00000", "-o", "merged", "-q"],
    ));
    let checked = run(root, &["verify-dir", "merged", "--json"]);
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(report["base_version"], "01.00");
    assert_eq!(report["complete"], true);
    assert_eq!(report["issues"], serde_json::json!([]));
    fs::write(root.join("merged/base.txt"), b"EDIT").unwrap();
    fs::remove_file(root.join("merged/sce_sys/param.sfo")).unwrap();
    fs::write(root.join("merged/extra"), b"EXTRA").unwrap();
    let checked = run(root, &["verify-dir", "merged", "--json"]);
    assert_eq!(checked.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(
        report["issues"],
        serde_json::json!([
            {"path":"base.txt","kind":"changed"}, {"path":"extra","kind":"unexpected"},
            {"path":"sce_sys/param.sfo","kind":"missing"}
        ])
    );
}

#[test]
fn refuses_unsafe_or_ambiguous_manifest_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    base_game(root);
    success(&run(
        root,
        &["export", "--base", "CUSA00000", "-o", "merged", "-q"],
    ));
    let path = root.join("merged/.orbis-export.json");
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for unsafe_path in [
        "../outside",
        "/absolute",
        "sce_sys/../escape",
        ".orbis-export.json",
        "BASE.TXT",
    ] {
        let mut manifest = original.clone();
        let mut extra = manifest["files"][0].clone();
        extra["path"] = unsafe_path.into();
        manifest["files"].as_array_mut().unwrap().push(extra);
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(run(root, &["verify-dir", "merged"]).status.code(), Some(1));
    }
}
