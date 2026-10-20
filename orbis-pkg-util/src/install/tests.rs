use super::*;
use crate::test_support::*;
use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicBool, Ordering},
};

fn package(version: &str, name: &str) -> Pkg<Vec<u8>> {
    Pkg::new(update(version, name, version.as_bytes())).unwrap()
}
fn active(base: &Path) -> PathBuf {
    base.with_file_name("CUSA00000-UPDATE")
}
fn manager(base: &Path) -> PathBuf {
    base.with_file_name(".CUSA00000.orbis-install")
}
fn version(base: &Path) -> String {
    let dir = Dir::open_ambient_dir(active(base), cap_std::ambient_authority()).unwrap();
    storage::app_metadata(&dir).unwrap().version.to_string()
}

#[test]
fn dry_run_is_read_only_and_reports_overlay_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    let p = plan(&package("01.01", "base.txt"), &base).unwrap();
    assert_eq!(p.current_version, "01.00");
    assert_eq!(p.update_version, "01.01");
    assert!(
        p.files
            .iter()
            .any(|f| f.path == "base.txt" && f.change == ChangeKind::Replace)
    );
    assert!(!manager(&base).exists());
    assert!(!active(&base).exists());
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
}

#[test]
fn upgrades_replace_entire_overlay_and_rollback_restores_previous() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    install(&package("01.01", "old.txt"), &base, SilentProgress).unwrap();
    let next = package("01.02", "new.txt");
    let p = plan(&next, &base).unwrap();
    assert!(
        p.files
            .iter()
            .any(|f| f.path == "old.txt" && f.change == ChangeKind::RemoveFromUpdate)
    );
    install(&next, &base, SilentProgress).unwrap();
    assert_eq!(version(&base), "01.02");
    assert!(!active(&base).join("old.txt").exists());
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    rollback(&base).unwrap();
    assert_eq!(version(&base), "01.01");
    assert!(active(&base).join("old.txt").exists());
    assert!(!active(&base).join("new.txt").exists());
    assert!(
        rollback(&base).is_err(),
        "rollback is not an indefinite toggle"
    );
}

#[test]
fn first_install_rolls_back_to_base_and_repeat_install_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    let p = package("01.01", "new.txt");
    install(&p, &base, SilentProgress).unwrap();
    assert!(
        install(&p, &base, SilentProgress)
            .unwrap_err()
            .to_string()
            .contains("not newer")
    );
    assert!(plan(&package("01.00", "older"), &base).is_err());
    rollback(&base).unwrap();
    assert!(!active(&base).exists());
    install(&p, &base, SilentProgress).unwrap();
    assert_eq!(version(&base), "01.01");
}

#[test]
fn rejects_wrong_titles_categories_and_delta_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    for (at, n) in [
        (0x74, 0x1e),
        (0x78, 0x41000000),
        (0x74, 0x1b),
        (0x78, 0x40000000),
    ] {
        let mut bytes = update("01.01", "file", b"new");
        be32(&mut bytes, at, n);
        assert!(plan(&Pkg::new(bytes).unwrap(), &base).is_err());
    }
    for (title, category) in [
        ("CUSA00001", "gp"),
        ("CUSA00000", "gd"),
        ("CUSA00000", "ac"),
    ] {
        let mut bytes = update("01.01", "file", b"new");
        let sfo = sfo(title, category, "01.01");
        bytes[0x1a00..0x1a00 + sfo.len()].copy_from_slice(&sfo);
        assert!(plan(&Pkg::new(bytes).unwrap(), &base).is_err());
    }
    assert!(!active(&base).exists());
    assert!(!manager(&base).exists());
}

#[test]
fn refuses_unmanaged_modified_and_conflicting_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    let pkg = package("01.01", "new.txt");
    fs::create_dir(active(&base)).unwrap();
    fs::write(active(&base).join("important"), b"KEEP").unwrap();
    assert!(install(&pkg, &base, SilentProgress).is_err());
    assert_eq!(fs::read(active(&base).join("important")).unwrap(), b"KEEP");
    fs::remove_dir_all(active(&base)).unwrap();
    install(&pkg, &base, SilentProgress).unwrap();
    fs::write(active(&base).join("new.txt"), b"MODIFIED").unwrap();
    assert!(rollback(&base).is_err());
    assert!(install(&package("01.02", "next"), &base, SilentProgress).is_err());
    assert_eq!(
        fs::read(active(&base).join("new.txt")).unwrap(),
        b"MODIFIED"
    );
    fs::create_dir(base.with_file_name("CUSA00000-patch")).unwrap();
    assert!(plan(&package("01.02", "next"), &base).is_err());
}

struct CancelOnFiles(AtomicBool);
impl ExtractProgress for CancelOnFiles {
    fn phase(&self, phase: &str) {
        if phase == "Extracting filesystem" {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
#[test]
fn cancellation_and_corruption_preserve_active_update() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    install(&package("01.01", "old"), &base, SilentProgress).unwrap();
    let error = install(
        &package("01.02", "next"),
        &base,
        CancelOnFiles(AtomicBool::new(false)),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        InstallError::Extract {
            source: ExtractError::Cancelled
        }
    ));
    assert_eq!(version(&base), "01.01");
    assert!(!manager(&base).join("staging").exists());

    // A malformed PFSC data block is readable during planning but fails extraction.
    let compressed = pfsc(4096, 4096, 4, &[80, 90]).0;
    let mut bad = update("01.02", "next", b"next");
    let outer = pfs("pfs_image.dat", &pfs("next", &compressed, true), false);
    bad.truncate(0x2000);
    bad.extend(&outer);
    be64(&mut bad, 0x418, outer.len() as u64);
    let bad = Pkg::new(bad).unwrap();
    assert!(install(&bad, &base, SilentProgress).is_err());
    assert_eq!(version(&base), "01.01");
    assert!(!manager(&base).join("staging").exists());
}

fn interrupted(at: &'static str, action: impl FnOnce()) {
    transaction::FAIL_AT.with(|f| f.set(Some(at)));
    let result = catch_unwind(AssertUnwindSafe(action));
    transaction::FAIL_AT.with(|f| f.set(None));
    assert!(result.is_err(), "checkpoint {at} was not reached");
}

#[test]
fn recovery_restores_prior_installation_at_every_activation_boundary() {
    for existing in [0, 1, 2] {
        for point in [
            "journal",
            "retired",
            "previous",
            "activated",
            "state",
            "committed",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let base = base_game(tmp.path());
            if existing >= 1 {
                install(&package("01.01", "one"), &base, SilentProgress).unwrap();
            }
            if existing >= 2 {
                install(&package("01.02", "two"), &base, SilentProgress).unwrap();
            }
            interrupted(point, || {
                let _ = install(&package("01.03", "three"), &base, SilentProgress);
            });
            recover(&base).unwrap();
            recover(&base).unwrap();
            if point == "committed" {
                assert_eq!(version(&base), "01.03");
                rollback(&base).unwrap();
            } else if existing == 0 {
                assert!(!active(&base).exists());
            } else {
                assert_eq!(
                    version(&base),
                    if existing == 1 { "01.01" } else { "01.02" }
                );
                rollback(&base).unwrap();
                if existing == 1 {
                    assert!(!active(&base).exists());
                } else {
                    assert_eq!(version(&base), "01.01");
                }
            }
            assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
        }
    }
}

#[test]
fn recovery_restores_prior_installation_at_every_rollback_boundary() {
    for existing in [1, 2] {
        for point in ["journal", "retired", "activated", "state", "committed"] {
            let tmp = tempfile::tempdir().unwrap();
            let base = base_game(tmp.path());
            install(&package("01.01", "one"), &base, SilentProgress).unwrap();
            if existing == 2 {
                install(&package("01.02", "two"), &base, SilentProgress).unwrap();
            }
            interrupted(point, || {
                let _ = rollback(&base);
            });
            recover(&base).unwrap();
            recover(&base).unwrap();
            if point == "committed" {
                if existing == 1 {
                    assert!(!active(&base).exists());
                } else {
                    assert_eq!(version(&base), "01.01");
                }
            } else {
                assert_eq!(
                    version(&base),
                    if existing == 1 { "01.01" } else { "01.02" }
                );
                rollback(&base).unwrap();
            }
        }
    }
}

#[test]
fn lock_prevents_simultaneous_installation_and_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    let guard = Layout::open(&base, true).unwrap();
    assert!(
        install(&package("01.01", "one"), &base, SilentProgress)
            .unwrap_err()
            .to_string()
            .contains("lock")
    );
    assert!(recover(&base).unwrap_err().to_string().contains("lock"));
    drop(guard);
    recover(&base).unwrap();
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_update_metadata_and_state() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("important"), b"KEEP").unwrap();
    symlink(&outside, active(&base)).unwrap();
    assert!(install(&package("01.01", "one"), &base, SilentProgress).is_err());
    fs::remove_file(active(&base)).unwrap();
    // The root may have been created during refusal, but no outside files changed.
    fs::remove_dir_all(manager(&base)).unwrap();
    symlink(&outside, manager(&base)).unwrap();
    assert!(recover(&base).is_err());
    assert_eq!(fs::read(outside.join("important")).unwrap(), b"KEEP");
}

#[test]
fn io_failure_during_activation_recovers_automatically() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    install(&package("01.01", "one"), &base, SilentProgress).unwrap();
    for point in ["journal", "retired", "previous", "activated", "state"] {
        transaction::ERROR_AT.with(|f| f.set(Some(point)));
        let error = install(&package("01.02", "two"), &base, SilentProgress).unwrap_err();
        transaction::ERROR_AT.with(|f| f.set(None));
        assert!(matches!(error, InstallError::Io { .. }));
        assert_eq!(version(&base), "01.01");
        assert!(!manager(&base).join("journal.json").exists());
        assert!(!manager(&base).join("staging").exists());
    }
    rollback(&base).unwrap();
    assert!(!active(&base).exists());
}

#[test]
fn protects_modified_backups_and_rejects_base_path_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    fs::create_dir(base.join("directory")).unwrap();
    assert!(plan(&package("01.01", "directory"), &base).is_err());
    install(&package("01.01", "one"), &base, SilentProgress).unwrap();
    install(&package("01.02", "two"), &base, SilentProgress).unwrap();
    fs::write(manager(&base).join("previous/one"), b"MODIFIED").unwrap();
    assert!(rollback(&base).is_err());
    assert!(install(&package("01.03", "three"), &base, SilentProgress).is_err());
    assert_eq!(
        fs::read(manager(&base).join("previous/one")).unwrap(),
        b"MODIFIED"
    );
    assert_eq!(version(&base), "01.02");
}

#[test]
fn rejects_unknown_metadata_reserved_paths_and_insufficient_space() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    let mut bytes = update("01.01", "file", b"NEW");
    be32(&mut bytes, 0x1000, 0xfeedcafe);
    assert!(plan(&Pkg::parse(bytes).unwrap(), &base).is_err());
    for name in [".orbis-install.json", "NUL", "bad.", "bad:stream"] {
        assert!(plan(&package("01.01", name), &base).is_err());
    }
    assert!(
        check_space(100, 99)
            .unwrap_err()
            .to_string()
            .contains("insufficient free space")
    );
    assert!(check_space(100, 100).is_ok());
    assert!(!active(&base).exists());
}
