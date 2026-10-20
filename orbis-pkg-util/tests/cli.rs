#![cfg(feature = "cli")]

use std::process::Command;

#[test]
fn documented_commands_have_help() {
    for command in ["extract", "info", "list"] {
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
