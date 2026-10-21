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
