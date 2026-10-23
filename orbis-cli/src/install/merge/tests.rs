use super::*;
use crate::test_support::*;
use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
};

fn package(name: &str) -> Pkg<Vec<u8>> {
    Pkg::new(update("01.01", name, b"PATCH")).unwrap()
}
fn manager(base: &Path) -> PathBuf {
    base.with_file_name(".CUSA00000.orbis-install")
}
fn snapshot(base: &Path) -> Vec<FileRecord> {
    inventory(
        &Dir::open_ambient_dir(base, cap_std::ambient_authority()).unwrap(),
        &SilentProgress,
    )
    .unwrap()
}
fn merge(base: &Path, name: &str) -> InstallPlan {
    install(&package(name), base, SilentProgress).unwrap()
}

#[test]
fn merges_only_patch_paths_and_restores_exact_originals() {
    for name in ["base.txt", "new.txt"] {
        let tmp = tempfile::tempdir().unwrap();
        let base = base_game(tmp.path());
        fs::write(base.join("untouched"), b"KEEP").unwrap();
        let original = snapshot(&base);
        let plan = plan(&package(name), &base, SilentProgress).unwrap();
        assert_eq!(plan.mode, InstallMode::Merge);
        assert_eq!(plan.destination, fs::canonicalize(&base).unwrap());
        assert!(plan.backup_bytes >= fs::metadata(base.join("sce_sys/param.sfo")).unwrap().len());
        assert!(!manager(&base).exists());
        merge(&base, name);
        assert_eq!(fs::read(base.join(name)).unwrap(), b"PATCH");
        assert_eq!(fs::read(base.join("untouched")).unwrap(), b"KEEP");
        assert!(!base.with_file_name("CUSA00000-UPDATE").exists());
        let status = super::super::status(&base, SilentProgress).unwrap();
        assert_eq!(status.mode, InstallMode::Merge);
        assert!(status.issues.is_empty(), "{:?}", status.issues);
        assert_eq!(status.active_version.as_deref(), Some("01.01"));
        super::super::recover(&base).unwrap(); // A committed merge stays installed.
        assert!(merge_status(&base).unwrap().is_some());
        super::super::rollback(&base).unwrap();
        assert_eq!(snapshot(&base), original);
        assert!(!manager(&base).join(ACTIVE).exists());
        super::super::recover(&base).unwrap();
        merge(&base, name); // Another merge is allowed after restoring.
    }
}

#[test]
fn rejects_mixed_modes_and_requires_restore_before_another_patch() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    merge(&base, "new.txt");
    let next = Pkg::new(update("01.02", "next.txt", b"NEXT")).unwrap();
    assert!(
        plan(&next, &base, SilentProgress)
            .unwrap_err()
            .to_string()
            .contains("restore")
    );
    assert!(
        super::super::plan(&next, &base)
            .unwrap_err()
            .to_string()
            .contains("restore")
    );
    assert!(super::super::plan_export(&base, &tmp.path().join("export"), SilentProgress).is_err());
    super::super::rollback(&base).unwrap();
    super::super::install(&package("new.txt"), &base, SilentProgress).unwrap();
    assert!(
        plan(&next, &base, SilentProgress)
            .unwrap_err()
            .to_string()
            .contains("separate update")
    );
}

#[test]
fn interrupted_apply_and_restore_recover_idempotently() {
    for point in [
        "journal",
        "backup",
        "publish",
        "committed",
        "restore-journal",
        "restore-unpublish",
        "restore-original",
        "restored",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let base = base_game(tmp.path());
        let original = snapshot(&base);
        if point.starts_with("restore") {
            merge(&base, "base.txt");
        }
        FAIL_AT.with(|f| f.set(Some(point)));
        let crashed = catch_unwind(AssertUnwindSafe(|| {
            if point.starts_with("restore") {
                super::super::rollback(&base).unwrap();
            } else {
                merge(&base, "base.txt");
            }
        }));
        assert!(crashed.is_err(), "did not reach {point}");
        for _ in 0..2 {
            super::super::recover(&base).unwrap();
        }
        if point == "committed" {
            assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"PATCH");
            super::super::rollback(&base).unwrap();
        }
        assert_eq!(snapshot(&base), original, "{point}");
        assert!(merge_status(&base).unwrap().is_none());
    }
}

#[test]
fn changed_game_or_backup_blocks_restore_before_any_files_move() {
    for relative in [
        "CUSA00000/base.txt",
        ".CUSA00000.orbis-install/merge/originals/base.txt",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let base = base_game(tmp.path());
        merge(&base, "base.txt");
        fs::write(tmp.path().join(relative), b"LOCAL CHANGES").unwrap();
        let before = snapshot(&base);
        assert!(
            super::super::rollback(&base)
                .unwrap_err()
                .to_string()
                .contains("file changed")
        );
        assert_eq!(snapshot(&base), before);
        assert!(!manager(&base).join("merge/pending.json").exists());
        assert!(
            !super::super::status(&base, SilentProgress)
                .unwrap()
                .issues
                .is_empty()
        );
    }
}

#[test]
fn cancellation_after_backup_restores_original_even_when_cancelled() {
    struct Cancel(PathBuf);
    impl ExtractProgress for Cancel {
        fn is_cancelled(&self) -> bool {
            self.0.exists()
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    let original = snapshot(&base);
    let error = install(
        &package("base.txt"),
        &base,
        Cancel(manager(&base).join("merge/originals/base.txt")),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        InstallError::Extract {
            source: ExtractError::Cancelled
        }
    ));
    assert_eq!(snapshot(&base), original);
    assert!(merge_status(&base).unwrap().is_none());
}

#[test]
fn incomplete_or_delta_update_never_touches_base() {
    for delta in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let base = base_game(tmp.path());
        let original = snapshot(&base);
        let mut bytes = update("01.01", "base.txt", b"PATCH");
        if delta {
            be32(&mut bytes, 0x74, 0x1e);
        } else {
            be32(&mut bytes, 0x10, 4);
            be32(&mut bytes, 0x1060, 0x1200);
            be32(&mut bytes, 0x1068, 0x8000_0000);
            be32(&mut bytes, 0x1070, 0x1f00);
            be32(&mut bytes, 0x1074, 16);
        }
        assert!(install(&Pkg::new(bytes).unwrap(), &base, SilentProgress).is_err());
        assert_eq!(snapshot(&base), original);
        assert!(merge_status(&base).unwrap().is_none());
    }
}

#[test]
fn refuses_case_collisions_and_file_directory_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    assert!(plan(&package("BASE.txt"), &base, SilentProgress).is_err());
    assert!(plan(&package("sce_sys"), &base, SilentProgress).is_err());
    assert!(!manager(&base).exists());
}

#[cfg(unix)]
#[test]
fn refuses_links_in_game_and_backup_trees() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    symlink(base.join("base.txt"), base.join("link")).unwrap();
    assert!(plan(&package("new.txt"), &base, SilentProgress).is_err());
    fs::remove_file(base.join("link")).unwrap();
    merge(&base, "base.txt");
    let backup = manager(&base).join("merge/originals/base.txt");
    fs::remove_file(&backup).unwrap();
    symlink(base.join("base.txt"), &backup).unwrap();
    assert!(super::super::rollback(&base).is_err());
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"PATCH");
}

#[test]
fn tampered_journal_paths_are_rejected_without_writing_outside_game() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    merge(&base, "base.txt");
    let path = manager(&base).join("merge/manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["files"][0]["after"]["path"] = "../outside".into();
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(super::super::rollback(&base).is_err());
    assert!(!tmp.path().join("outside").exists());
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"PATCH");
}

#[test]
fn recovery_keeps_an_unexpected_file_instead_of_overwriting_it() {
    let tmp = tempfile::tempdir().unwrap();
    let base = base_game(tmp.path());
    FAIL_AT.with(|f| f.set(Some("backup")));
    assert!(catch_unwind(AssertUnwindSafe(|| merge(&base, "base.txt"))).is_err());
    fs::write(base.join("base.txt"), b"USER FILE").unwrap();
    assert!(super::super::recover(&base).is_err());
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"USER FILE");
    assert_eq!(
        fs::read(manager(&base).join("merge/originals/base.txt")).unwrap(),
        b"BASE"
    );
    fs::remove_file(base.join("base.txt")).unwrap();
    super::super::recover(&base).unwrap();
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
}

#[test]
fn every_file_move_can_be_recovered_after_a_crash() {
    let pkg = package("base.txt");
    for (point, count) in [
        ("backup", 2),
        ("publish", 4),
        ("restore-unpublish", 4),
        ("restore-original", 2),
    ] {
        for skip in 0..count {
            let tmp = tempfile::tempdir().unwrap();
            let base = base_game(tmp.path());
            let original = snapshot(&base);
            if point.starts_with("restore") {
                install(&pkg, &base, SilentProgress).unwrap();
            }
            FAIL_AT.with(|f| f.set(Some(point)));
            FAIL_SKIP.with(|f| f.set(skip));
            let crashed = catch_unwind(AssertUnwindSafe(|| {
                if point.starts_with("restore") {
                    super::super::rollback(&base).unwrap();
                } else {
                    install(&pkg, &base, SilentProgress).unwrap();
                }
            }));
            FAIL_AT.with(|f| f.set(None));
            FAIL_SKIP.with(|f| f.set(0));
            assert!(crashed.is_err(), "did not reach {point} {skip}");
            super::super::recover(&base).unwrap();
            super::super::recover(&base).unwrap();
            assert_eq!(snapshot(&base), original, "{point} {skip}");
        }
    }
}

fn nested_package() -> Pkg<Vec<u8>> {
    // root -> uroot -> folder -> file + empty directory. Independent PFS bytes
    // exercise real extraction, including directories absent from the base.
    let mut image = pfs("folder", b"PATCH", false);
    image.resize(7 * BS, 0);
    le64(&mut image, 0x28, 7);
    le64(&mut image, 0x30, 5);
    image[BS + 2 * 168..BS + 2 * 168 + 2].copy_from_slice(&0x4000u16.to_le_bytes());
    le64(&mut image, BS + 2 * 168 + 8, BS as u64);
    le64(&mut image, BS + 2 * 168 + 16, BS as u64);
    dirent(&mut image[3 * BS..], 2, 3, "folder");
    image[4 * BS..5 * BS].fill(0);
    for (inode, directory, block, size) in [(3, false, 5, 5), (4, true, 6, BS)] {
        let at = BS + inode * 168;
        image[at..at + 2]
            .copy_from_slice(&(if directory { 0x4000u16 } else { 0x8000u16 }).to_le_bytes());
        le64(&mut image, at + 8, size as u64);
        le64(&mut image, at + 16, size as u64);
        le32(&mut image, at + 96, 1);
        le32(&mut image, at + 100, block);
        le32(&mut image, at + 104, u32::MAX);
    }
    dirent(&mut image[4 * BS..], 3, 2, "file");
    dirent(&mut image[4 * BS + 20..], 4, 3, "empty");
    image[5 * BS..5 * BS + 5].copy_from_slice(b"PATCH");
    let mut bytes = pkg(image);
    let meta = sfo("CUSA00000", "gp", "01.01");
    bytes[0x1a00..0x1a00 + meta.len()].copy_from_slice(&meta);
    be32(&mut bytes, 0x1054, meta.len() as u32);
    be32(&mut bytes, 0x74, 0x1a);
    be32(&mut bytes, 0x78, 0x60000000);
    Pkg::new(bytes).unwrap()
}

#[test]
fn new_nested_directories_are_removed_only_when_empty() {
    let pkg = nested_package();
    for preserve_new_file in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let base = base_game(tmp.path());
        let original = snapshot(&base);
        install(&pkg, &base, SilentProgress).unwrap();
        assert_eq!(fs::read(base.join("folder/file")).unwrap(), b"PATCH");
        assert!(base.join("folder/empty").is_dir());
        if preserve_new_file {
            fs::write(base.join("folder/empty/user"), b"KEEP").unwrap();
        }
        super::super::rollback(&base).unwrap();
        assert!(!base.join("folder/file").exists());
        if preserve_new_file {
            assert_eq!(fs::read(base.join("folder/empty/user")).unwrap(), b"KEEP");
        } else {
            assert!(!base.join("folder").exists());
            assert_eq!(snapshot(&base), original);
        }
    }
}
