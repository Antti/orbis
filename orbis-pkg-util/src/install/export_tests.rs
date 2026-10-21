use super::*;
use crate::{SilentProgress, install, test_support::*};
use orbis_pkg::Pkg;
use std::{
    fs,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

fn install_update(base: &Path, name: &str, bytes: &[u8]) {
    install::install(
        &Pkg::new(update("01.09", name, bytes)).unwrap(),
        base,
        SilentProgress,
    )
    .unwrap();
}

fn active(base: &Path) -> PathBuf {
    base.with_file_name("CUSA00000-UPDATE")
}

fn no_staging(parent: &Path) {
    assert!(!fs::read_dir(parent).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".orbis-export-")
    }));
}

fn publication_fixture() -> (tempfile::TempDir, Stage, BTreeMap<String, Entry>) {
    let temp = tempfile::tempdir().unwrap();
    let parent = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
    let stage = Stage::new(parent, "merged".into()).unwrap();
    stage
        .dir
        .as_ref()
        .unwrap()
        .write("file", b"CONTENTS")
        .unwrap();
    let entries = BTreeMap::from([(
        "file".into(),
        Entry {
            path: "file".into(),
            layer: Layer::Base,
            directory: false,
            bytes: 8,
        },
    )]);
    (temp, stage, entries)
}

#[cfg(unix)]
#[test]
fn staging_cleanup_preserves_replaced_directories_and_symlinks() {
    for symlink in [false, true] {
        let (temp, mut stage, _) = publication_fixture();
        stage.dir.take();
        let staged = temp.path().join(&stage.temporary);
        fs::rename(&staged, temp.path().join("moved")).unwrap();
        if symlink {
            let other = temp.path().join("other");
            fs::create_dir(&other).unwrap();
            fs::write(other.join("keep"), b"KEEP").unwrap();
            std::os::unix::fs::symlink(other, &staged).unwrap();
        } else {
            fs::create_dir(&staged).unwrap();
            fs::write(staged.join("keep"), b"KEEP").unwrap();
        }
        drop(stage);
        assert_eq!(fs::read(staged.join("keep")).unwrap(), b"KEEP");
    }
}

fn unsupported_rename(_: &Dir, _: &str, _: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    return Err(std::io::Error::from_raw_os_error(
        rustix::io::Errno::NOTSUP.raw_os_error(),
    ));
    #[cfg(not(target_os = "macos"))]
    Err(std::io::ErrorKind::Unsupported.into())
}

#[test]
fn unsupported_directory_rename_publishes_the_complete_export() {
    let (temp, stage, entries) = publication_fixture();
    // Filesystem-generated metadata is not part of the planned game tree.
    stage
        .dir
        .as_ref()
        .unwrap()
        .write("unplanned", b"METADATA")
        .unwrap();
    stage
        .publish_with(&entries, 8, &SilentProgress, unsupported_rename)
        .unwrap();
    assert_eq!(
        fs::read(temp.path().join("merged/file")).unwrap(),
        b"CONTENTS"
    );
    assert!(!temp.path().join("merged/unplanned").exists());
    no_staging(temp.path());
}

#[test]
fn publication_preserves_destinations_created_before_fallback() {
    let (temp, stage, entries) = publication_fixture();
    assert!(
        stage
            .publish_with(
                &entries,
                8,
                &SilentProgress,
                |parent, temporary, destination| {
                    parent.create_dir(destination)?;
                    unsupported_rename(parent, temporary, destination)
                }
            )
            .is_err()
    );
    assert!(temp.path().join("merged").is_dir());
    assert_eq!(fs::read_dir(temp.path().join("merged")).unwrap().count(), 0);
    no_staging(temp.path());
}

#[test]
fn publication_does_not_fallback_for_permission_errors() {
    let (temp, stage, entries) = publication_fixture();
    assert!(
        stage
            .publish_with(&entries, 8, &SilentProgress, |_, _, _| {
                Err(std::io::ErrorKind::PermissionDenied.into())
            })
            .is_err()
    );
    assert!(!temp.path().join("merged").exists());
    no_staging(temp.path());
}

#[test]
fn cancellation_during_fallback_cleans_both_output_and_staging() {
    struct Cancel(AtomicBool);
    impl ExtractProgress for Cancel {
        fn bytes_processed(&self, bytes: u64) {
            if bytes > 0 {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    let (temp, stage, entries) = publication_fixture();
    let error = stage
        .publish_with(
            &entries,
            8,
            &Cancel(AtomicBool::new(false)),
            unsupported_rename,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        install::InstallError::Extract {
            source: ExtractError::Cancelled
        }
    ));
    assert!(!temp.path().join("merged").exists());
    no_staging(temp.path());
}

#[test]
fn exports_to_a_configured_external_filesystem() {
    let Some(root) = std::env::var_os("ORBIS_TEST_EXPORT_ROOT") else {
        return;
    };
    // Keep source fixtures local and touch only a private subtree on the volume.
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir_in(root).unwrap();
    let base = base_game(source.path());
    fs::create_dir_all(base.join("assets/empty")).unwrap();
    install_update(&base, "new", b"UPDATE");
    let output = target.path().join("merged");
    let report = export_merged(&base, &output, SilentProgress).unwrap();
    assert_eq!(report.update_files, 4);
    assert_eq!(fs::read(output.join("base.txt")).unwrap(), b"BASE");
    assert_eq!(fs::read(output.join("new")).unwrap(), b"UPDATE");
    assert_eq!(
        fs::read(output.join("sce_sys/param.sfo")).unwrap(),
        sfo("CUSA00000", "gp", "01.09")
    );
    assert!(output.join("assets/empty").is_dir());
    assert!(!output.join(storage::MANIFEST).exists());
    assert!(export_merged(&base, &output, SilentProgress).is_err());
    assert_eq!(fs::read(output.join("new")).unwrap(), b"UPDATE");
    no_staging(target.path());
}

#[test]
fn merges_update_with_base_fallback_and_preserves_rollback() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    fs::create_dir_all(base.join("assets/empty")).unwrap();
    fs::write(base.join("assets/unchanged"), b"ASSET").unwrap();
    fs::write(base.join("eboot.bin"), b"OLD BOOT").unwrap();
    install_update(&base, "eboot.bin", b"NEW BOOT");
    let output = temp.path().join("merged");
    let report = export_merged(&base, &output, SilentProgress).unwrap();
    assert_eq!(report.title_id, "CUSA00000");
    assert_eq!(report.app_version, "01.09");
    assert_eq!(report.files, 6); // Three fixture update entries, plus eboot, asset, base.txt.
    assert_eq!(report.update_files, 4);
    assert!(output.join("assets/empty").is_dir());
    for (path, bytes) in [
        ("eboot.bin", b"NEW BOOT".as_slice()),
        ("base.txt", b"BASE"),
        ("assets/unchanged", b"ASSET"),
    ] {
        assert_eq!(fs::read(output.join(path)).unwrap(), bytes);
    }
    assert_eq!(
        fs::read(output.join("sce_sys/param.sfo")).unwrap(),
        sfo("CUSA00000", "gp", "01.09")
    );
    assert!(!output.join(storage::MANIFEST).exists());
    assert_eq!(fs::read(base.join("eboot.bin")).unwrap(), b"OLD BOOT");
    let actual_bytes: u64 = [
        "eboot.bin",
        "base.txt",
        "assets/unchanged",
        "sce_sys/param.sfo",
        "sce_sys/entry_keys",
        "sce_sys/image_key",
    ]
    .iter()
    .map(|path| fs::metadata(output.join(path)).unwrap().len())
    .sum();
    assert_eq!(report.bytes, actual_bytes);
    // Editing the export must not modify either source through hard links.
    fs::write(output.join("eboot.bin"), b"EDITED EXPORT").unwrap();
    fs::write(output.join("base.txt"), b"EDITED EXPORT").unwrap();
    assert_eq!(
        fs::read(active(&base).join("eboot.bin")).unwrap(),
        b"NEW BOOT"
    );
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    install::rollback(&base).unwrap();
    assert!(!active(&base).exists());
    no_staging(temp.path());
}

#[test]
fn exports_base_alone_and_current_update_only() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    let report = export_merged(&base, &temp.path().join("base-copy"), SilentProgress).unwrap();
    assert_eq!(
        (
            report.app_version.as_str(),
            report.files,
            report.update_files
        ),
        ("01.00", 2, 0)
    );
    install::install(
        &Pkg::new(update("01.01", "obsolete", b"OLD")).unwrap(),
        &base,
        SilentProgress,
    )
    .unwrap();
    install_update(&base, "new", b"NEW");
    let output = temp.path().join("current");
    export_merged(&base, &output, SilentProgress).unwrap();
    assert!(!output.join("obsolete").exists());
    assert_eq!(fs::read(output.join("new")).unwrap(), b"NEW");
    install::rollback(&base).unwrap();
    assert!(active(&base).join("obsolete").exists());
}

#[test]
fn refuses_existing_or_overlapping_outputs_without_changing_them() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    install_update(&base, "new", b"NEW");
    let existing = temp.path().join("existing");
    fs::create_dir(&existing).unwrap();
    let file = temp.path().join("file");
    fs::write(&file, b"KEEP").unwrap();
    for path in [
        existing.clone(),
        file.clone(),
        base.clone(),
        base.join("child"),
        active(&base).join("child"),
        temp.path().join(".CUSA00000.orbis-install/child"),
        temp.path().join("missing-parent/child"),
    ] {
        assert!(
            export_merged(&base, &path, SilentProgress).is_err(),
            "{}",
            path.display()
        );
    }
    assert!(existing.is_dir());
    assert_eq!(fs::read(&file).unwrap(), b"KEEP");
    assert!(!base.join("child").exists());
    no_staging(temp.path());
}

#[test]
fn refuses_unmanaged_modified_pending_and_locked_installations() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    let output = temp.path().join("merged");
    fs::create_dir(active(&base)).unwrap();
    assert!(export_merged(&base, &output, SilentProgress).is_err());
    fs::remove_dir(active(&base)).unwrap();
    install_update(&base, "new", b"NEW");
    let lock = storage::Layout::open(&base, true).unwrap();
    assert!(
        export_merged(&base, &output, SilentProgress)
            .unwrap_err()
            .to_string()
            .contains("lock")
    );
    drop(lock);
    let journal = temp.path().join(".CUSA00000.orbis-install/journal.json");
    fs::write(&journal, b"interrupted").unwrap();
    assert!(
        export_merged(&base, &output, SilentProgress)
            .unwrap_err()
            .to_string()
            .contains("recover --base")
    );
    fs::remove_file(journal).unwrap();
    fs::write(active(&base).join("new"), b"MODIFIED").unwrap();
    assert!(
        export_merged(&base, &output, SilentProgress)
            .unwrap_err()
            .to_string()
            .contains("changed")
    );
    assert!(!output.exists());
    no_staging(temp.path());
}

#[test]
fn rejects_file_directory_and_cross_layer_case_conflicts() {
    for directory in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let base = base_game(temp.path());
        install_update(&base, "asset", b"UPDATE");
        if directory {
            fs::create_dir(base.join("asset")).unwrap();
        } else {
            fs::write(base.join("ASSET"), b"BASE").unwrap();
        }
        let output = temp.path().join("merged");
        let message = export_merged(&base, &output, SilentProgress)
            .unwrap_err()
            .to_string();
        assert!(
            message.contains(if directory {
                "file/directory conflict"
            } else {
                "case-colliding"
            }),
            "{message}"
        );
        assert!(!output.exists());
        no_staging(temp.path());
    }
}

struct DuringCopy {
    action: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    cancel: bool,
    copying: AtomicBool,
    cancelled: AtomicBool,
}

impl DuringCopy {
    fn new(cancel: bool, action: impl FnOnce() + Send + 'static) -> Self {
        Self {
            action: Mutex::new(Some(Box::new(action))),
            cancel,
            copying: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        }
    }
}

impl ExtractProgress for DuringCopy {
    fn phase(&self, phase: &str) {
        self.copying
            .store(phase == "Exporting merged game", Ordering::Relaxed);
    }
    fn bytes_processed(&self, _: u64) {
        if self.copying.load(Ordering::Relaxed)
            && let Some(action) = self.action.lock().unwrap().take()
        {
            action();
            self.cancelled.store(self.cancel, Ordering::Relaxed);
        }
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

#[test]
fn cancellation_discards_staging_and_releases_install_lock() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    let output = temp.path().join("merged");
    let error = export_merged(&base, &output, DuringCopy::new(true, || {})).unwrap_err();
    assert!(matches!(
        error,
        install::InstallError::Extract {
            source: ExtractError::Cancelled
        }
    ));
    assert!(!output.exists());
    no_staging(temp.path());
    export_merged(&base, &output, SilentProgress).unwrap();
}

#[test]
fn destination_created_during_copy_is_never_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    let output = temp.path().join("merged");
    let concurrent = output.clone();
    let error = export_merged(
        &base,
        &output,
        DuringCopy::new(false, move || {
            fs::create_dir(concurrent).unwrap(); // Even an empty directory must survive.
        }),
    )
    .unwrap_err();
    assert!(matches!(error, install::InstallError::Io { .. }));
    assert!(output.is_dir());
    assert_eq!(fs::read_dir(output).unwrap().count(), 0);
    no_staging(temp.path());
}

#[test]
fn copy_error_discards_completed_files_and_leaves_sources_intact() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    let output = temp.path().join("merged");
    let removed = base.join("z-file");
    fs::write(&removed, b"LATER").unwrap();
    assert!(
        export_merged(
            &base,
            &output,
            DuringCopy::new(false, move || {
                fs::remove_file(removed).unwrap();
            })
        )
        .is_err()
    );
    assert!(!output.exists());
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    no_staging(temp.path());
}

#[test]
fn rejects_case_aliases_of_source_directories_on_case_insensitive_volumes() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    install_update(&base, "new", b"NEW");
    for alias in ["cusa00000", "cusa00000-update", ".cusa00000.ORBIS-INSTALL"] {
        let alias = temp.path().join(alias);
        if !alias.is_dir() {
            continue; // Case-sensitive volumes do not have this alias.
        }
        let output = alias.join("merged");
        assert!(
            export_merged(&base, &output, SilentProgress)
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
        assert!(!output.exists());
        no_staging(&alias);
    }
}

#[test]
fn detects_update_changed_after_inventory_and_before_copying() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    install_update(&base, "z-file", b"UPDATE");
    let output = temp.path().join("merged");
    let changed = active(&base).join("z-file");
    let error = export_merged(
        &base,
        &output,
        DuringCopy::new(false, move || {
            // Same length: the copy must be checked against the recorded hash.
            fs::write(changed, b"EDITED").unwrap();
        }),
    )
    .unwrap_err();
    assert!(error.to_string().contains("changed during export"));
    assert!(!output.exists());
    no_staging(temp.path());
}

#[cfg(unix)]
#[test]
fn rejects_symlink_sources_and_destinations() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    let output = temp.path().join("merged");
    symlink("base.txt", base.join("linked")).unwrap();
    assert!(
        export_merged(&base, &output, SilentProgress)
            .unwrap_err()
            .to_string()
            .contains("symlinks")
    );
    fs::remove_file(base.join("linked")).unwrap();
    symlink(&base, temp.path().join("alias")).unwrap();
    assert!(export_merged(&base, &temp.path().join("alias/nested"), SilentProgress).is_err());
    symlink("missing", &output).unwrap();
    assert!(export_merged(&base, &output, SilentProgress).is_err());
    assert_eq!(fs::read_link(output).unwrap(), Path::new("missing"));
    no_staging(temp.path());
}
