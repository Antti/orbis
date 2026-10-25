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

#[test]
fn export_verification_rejects_a_rewritten_manifest_and_payload() {
    for resume in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let base = base_game(temp.path());
        let output = temp.path().join("merged");
        let exported = export_for_verification(&base, &output, resume, SilentProgress).unwrap();
        assert!(exported.verify(SilentProgress).unwrap().issues.is_empty());
        fs::write(output.join("base.txt"), b"EVIL").unwrap();
        let manifest_path = output.join(install::directory::MANIFEST);
        let mut manifest: install::directory::ExportManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        let file = manifest
            .files
            .iter_mut()
            .find(|file| file.path == "base.txt")
            .unwrap();
        file.sha256 = format!("{:x}", Sha256::digest(b"EVIL"));
        fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        // A self-consistent replacement inventory passes standalone verify-dir,
        // but cannot redefine the expected contents of this export.
        assert!(
            install::verify_directory(&output, SilentProgress)
                .unwrap()
                .issues
                .is_empty()
        );
        assert!(
            exported
                .verify(SilentProgress)
                .unwrap_err()
                .to_string()
                .contains("export manifest changed")
        );
        assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    }
}

#[cfg(unix)]
#[test]
fn export_verification_rejects_directory_replacement_before_and_during_verification() {
    struct Replace {
        output: PathBuf,
        moved: PathBuf,
        replacement: PathBuf,
    }
    impl Replace {
        fn swap(&self) {
            fs::rename(&self.output, &self.moved).unwrap();
            fs::rename(&self.replacement, &self.output).unwrap();
        }
    }
    impl ExtractProgress for Replace {
        fn phase(&self, name: &str) {
            if name == "Verifying exported files" {
                self.swap();
            }
        }
    }
    for resume in [false, true] {
        for during in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let base = base_game(temp.path());
            let progress = Replace {
                output: temp.path().join("merged"),
                moved: temp.path().join("moved"),
                replacement: temp.path().join("replacement"),
            };
            // Even identical contents must not let another directory impersonate
            // the one whose identity was retained by the export.
            export_merged(&base, &progress.replacement, SilentProgress).unwrap();
            let exported =
                export_for_verification(&base, &progress.output, resume, SilentProgress).unwrap();
            let error = if during {
                exported.verify(&progress).unwrap_err()
            } else {
                progress.swap();
                exported.verify(SilentProgress).unwrap_err()
            };
            assert!(error.to_string().contains("export destination changed"));
            assert_eq!(fs::read(progress.moved.join("base.txt")).unwrap(), b"BASE");
            assert_eq!(fs::read(progress.output.join("base.txt")).unwrap(), b"BASE");
        }
    }
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
    let plan = plan_export(&base, &output, SilentProgress).unwrap();
    if let Ok(expected) = std::env::var("ORBIS_TEST_FILESYSTEM") {
        assert_eq!(plan.filesystem.filesystem, expected);
    }
    let report = export_merged(&base, &output, SilentProgress).unwrap();
    assert!(
        install::verify_directory(&output, SilentProgress)
            .unwrap()
            .issues
            .is_empty()
    );
    let resumable = target.path().join("resumable");
    resume_export(&base, &resumable, SilentProgress).unwrap();
    fs::remove_file(resumable.join("base.txt")).unwrap();
    resume_export(&base, &resumable, SilentProgress).unwrap();
    assert!(
        install::verify_directory(&resumable, SilentProgress)
            .unwrap()
            .issues
            .is_empty()
    );
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

#[test]
fn cancelled_resumable_export_keeps_manifest_and_can_continue() {
    struct CancelCopy {
        copying: AtomicBool,
        cancelled: AtomicBool,
    }
    impl ExtractProgress for CancelCopy {
        fn phase(&self, phase: &str) {
            self.copying
                .store(phase == "Resuming merged export", Ordering::SeqCst);
        }
        fn bytes_processed(&self, _: u64) {
            if self.copying.load(Ordering::SeqCst) {
                self.cancelled.store(true, Ordering::SeqCst);
            }
        }
        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    let out = temp.path().join("resumable");
    assert!(
        resume_export(
            &base,
            &out,
            CancelCopy {
                copying: AtomicBool::new(false),
                cancelled: AtomicBool::new(false)
            }
        )
        .is_err()
    );
    let report = install::verify_directory(&out, SilentProgress).unwrap();
    assert!(!report.complete);
    assert!(!report.issues.is_empty());
    resume_export(&base, &out, SilentProgress).unwrap();
    let report = install::verify_directory(&out, SilentProgress).unwrap();
    assert!(report.complete && report.issues.is_empty());
}

#[test]
fn resume_rejects_case_alias_of_an_export_used_as_its_base() {
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    let source = temp.path().join("source-export");
    export_merged(&base, &source, SilentProgress).unwrap();
    let alias = temp.path().join("SOURCE-EXPORT");
    if !alias.is_dir() {
        return;
    } // The alias only exists on case-insensitive filesystems.
    let manifest = source.join(super::super::directory::MANIFEST);
    let before = fs::metadata(&manifest).unwrap().modified().unwrap();
    let error = resume_export(&source, &alias, SilentProgress).unwrap_err();
    assert!(error.to_string().contains("outside"));
    assert_eq!(fs::metadata(&manifest).unwrap().modified().unwrap(), before);
    assert!(!temp.path().join(".source-export.orbis-install").exists());
}

#[test]
fn preview_staged_and_resumable_exports_agree_on_the_overlay() {
    for with_update in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let base = base_game(temp.path());
        fs::create_dir(base.join("empty")).unwrap();
        if with_update {
            let pkg = Pkg::new(update("01.09", "base.txt", b"UPDATE")).unwrap();
            install::install(&pkg, &base, SilentProgress).unwrap();
        }
        let staged = temp.path().join("staged");
        let resumed = temp.path().join("resumed");
        let plan = plan_export(&base, &staged, SilentProgress).unwrap();
        let report = export_merged(&base, &staged, SilentProgress).unwrap();
        let resume = resume_export(&base, &resumed, SilentProgress).unwrap();
        for exported in [report, resume] {
            assert_eq!(exported.title_id, plan.title_id);
            assert_eq!(exported.files, plan.files);
            assert_eq!(exported.update_files, plan.update_files);
            assert_eq!(exported.bytes, plan.bytes);
            assert_eq!(
                exported.app_version,
                plan.update_version
                    .as_ref()
                    .unwrap_or(&plan.base_version)
                    .as_str()
            );
        }
        let manifest = super::super::directory::MANIFEST;
        assert_eq!(
            fs::read(staged.join(manifest)).unwrap(),
            fs::read(resumed.join(manifest)).unwrap()
        );
        assert!(staged.join("empty").is_dir());
        assert!(resumed.join("empty").is_dir());
    }
}

#[cfg(unix)]
#[test]
fn resume_keeps_the_validated_parent_when_its_path_is_replaced() {
    struct MoveParent {
        parent: PathBuf,
        moved: PathBuf,
    }
    impl ExtractProgress for MoveParent {
        fn phase(&self, phase: &str) {
            if phase == "Hashing export sources for safe resume" {
                fs::rename(&self.parent, &self.moved).unwrap();
                fs::create_dir(&self.parent).unwrap();
                fs::write(self.parent.join("keep"), b"KEEP").unwrap();
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let base = base_game(temp.path());
    let parent = temp.path().join("output-parent");
    let moved = temp.path().join("moved-parent");
    fs::create_dir(&parent).unwrap();
    resume_export(
        &base,
        &parent.join("merged"),
        MoveParent {
            parent: parent.clone(),
            moved: moved.clone(),
        },
    )
    .unwrap();
    assert!(!parent.join("merged").exists());
    assert_eq!(fs::read(parent.join("keep")).unwrap(), b"KEEP");
    let verified = install::verify_directory(&moved.join("merged"), SilentProgress).unwrap();
    assert!(verified.complete && verified.issues.is_empty());
}
