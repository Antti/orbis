use super::*;
use crate::worker::Progress;
use orbis_cli::{ExtractProgress, SilentProgress};
use std::{fs, sync::atomic::Ordering};

use crate::{common, features};

fn package(root: &Path) -> PathBuf {
    let path = root.join("game.pkg");
    fs::write(&path, common::update("01.01", "new.txt", b"NEW DATA")).unwrap();
    path
}

fn verify_report(
    path: &Path,
    metadata_only: bool,
    progress: &impl ExtractProgress,
) -> Box<VerificationReport> {
    let Outcome::Verified(report) =
        run(path, Operation::Verify { metadata_only }, progress).unwrap()
    else {
        panic!()
    };
    report
}

#[test]
fn skipped_readability_makes_a_report_incomplete_even_when_all_hashes_pass() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.pkg");
    fs::write(&path, features::signed_pkg_with_metadata(true)).unwrap();
    for metadata_only in [true, false] {
        let report = verify_report(&path, metadata_only, &SilentProgress);
        assert_eq!(report.status, "incomplete");
        assert_eq!(report.summary.unavailable, 0);
        assert_eq!(report.summary.failed, 0);
        assert!(report.summary.digests > 0);
        assert!(report.error.is_none());
        let json = serde_json::to_value(&report).unwrap();
        let skipped = json["readability"]["skipped"].as_array().unwrap();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0]["path"], "sce_sys/param.sfo");
        assert!(skipped[0]["reason"].as_str().unwrap().contains("slot 2"));
    }
}

#[test]
fn desktop_verification_distinguishes_complete_incomplete_failed_and_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signed.pkg");
    let bytes = features::signed_pkg();
    fs::write(&path, &bytes).unwrap();
    for metadata_only in [true, false] {
        let report = verify_report(&path, metadata_only, &SilentProgress);
        assert_eq!(report.status, "complete");
        assert!(report.summary.digests > 0);
        assert_eq!(report.summary.unavailable, 0);
        assert!(report.readability.as_ref().unwrap().skipped.is_empty());
        assert_eq!(report.metadata_only, metadata_only);
        assert!(!report.authenticity_checked);
    }
    let mut missing = bytes.clone();
    missing[0x100..0x180].fill(0);
    missing[0xfe0..0x1000].fill(0);
    fs::write(&path, missing).unwrap();
    let report = verify_report(&path, false, &SilentProgress);
    assert_eq!(report.status, "incomplete");
    assert!(report.summary.unavailable > 0);
    assert!(!report.unavailable_reasons.is_empty());

    let mut corrupt = bytes;
    corrupt[0x1a50] ^= 1;
    fs::write(&path, corrupt).unwrap();
    let report = verify_report(&path, false, &SilentProgress);
    assert_eq!(report.status, "failed");
    assert_eq!(report.summary.failed, 1);
    assert!(report.error.as_ref().unwrap().contains("SHA-256 mismatch"));
    let progress = Progress::default();
    progress.cancelled.store(true, Ordering::Relaxed);
    assert_eq!(verify_report(&path, false, &progress).status, "interrupted");
    assert_eq!(
        verify_report(&dir.path().join("absent.pkg"), false, &SilentProgress).status,
        "failed"
    );
}

#[test]
fn metadata_verification_excludes_game_data_and_reports_save_without_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.pkg");
    let mut bytes = features::signed_pkg();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&path, &bytes).unwrap();
    let report = verify_report(&path, true, &SilentProgress);
    assert_eq!(report.status, "complete");
    assert_eq!(
        verify_report(&path, false, &SilentProgress).status,
        "failed"
    );
    assert!(report.save(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let destination = dir.path().join("report.json");
    report.save(&destination).unwrap();
    assert!(report.save(&destination).is_err());
    let json: serde_json::Value = serde_json::from_slice(&fs::read(destination).unwrap()).unwrap();
    assert_eq!(json["status"], "complete");
    assert_eq!(json["metadata_only"], true);
    assert!(json.get("checks").is_none());
}

#[test]
fn passcode_unlocks_inspection_and_extraction_and_wrong_values_never_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.pkg");
    fs::write(&path, features::passcoded_pkg(2)).unwrap();
    let Outcome::Inspected(locked) = run(&path, Operation::Inspect, &SilentProgress).unwrap()
    else {
        panic!()
    };
    assert_eq!(locked.missing_keys, 1);
    let passcode = Passcode::new(
        String::from_utf8(features::PASSCODE.to_vec())
            .unwrap()
            .into(),
    )
    .unwrap();
    assert!(!format!("{passcode:?}").contains("0123456789abcdef"));
    let Outcome::Inspected(unlocked) =
        run_with_passcode(&path, Operation::Inspect, Some(&passcode), &SilentProgress).unwrap()
    else {
        panic!()
    };
    assert_eq!(unlocked.missing_keys, 0);
    assert_eq!(unlocked.version.as_deref(), Some("01.12"));
    let output = dir.path().join("output");
    let result = run_with_passcode(
        &path,
        Operation::Extract {
            output: output.clone(),
            resume: false,
        },
        Some(&passcode),
        &SilentProgress,
    )
    .unwrap();
    assert!(matches!(
        result,
        Outcome::Finished {
            incomplete: false,
            ..
        }
    ));
    assert!(output.join("sce_sys/param.sfo").is_file());
    let bad = Passcode::new("x".repeat(32).into()).unwrap();
    let untouched = dir.path().join("bad-output");
    let error = run_with_passcode(
        &path,
        Operation::Extract {
            output: untouched.clone(),
            resume: false,
        },
        Some(&bad),
        &SilentProgress,
    )
    .unwrap_err();
    assert!(error.details.contains("passcode"));
    assert!(!error.details.contains(&"x".repeat(32)));
    assert!(!untouched.exists());
    let Outcome::Verified(report) = run_with_passcode(
        &path,
        Operation::Verify {
            metadata_only: true,
        },
        Some(&passcode),
        &SilentProgress,
    )
    .unwrap() else {
        panic!()
    };
    assert!(report.readability.as_ref().unwrap().skipped.is_empty());
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("0123456789abcdef")
    );
    for bad in ["short".to_owned(), "é".repeat(16), "\n".repeat(32)] {
        assert!(Passcode::new(bad.into()).is_err());
    }
}

fn export_preview(base: &Path, output: &Path, resume: bool) -> Box<install::ExportPlan> {
    let Outcome::ExportPreview(plan) = run(
        Path::new(""),
        Operation::PreviewExport {
            base: base.into(),
            output: output.into(),
            resume,
        },
        &SilentProgress,
    )
    .unwrap() else {
        panic!()
    };
    plan
}

#[test]
fn export_previews_resumes_and_verifies_without_a_package_or_source_changes() {
    let dir = tempfile::tempdir().unwrap();
    let base = common::base_game(dir.path());
    let pkg = orbis_pkg::Pkg::parse(common::update("01.12", "base.txt", b"UPDATED")).unwrap();
    install::install(&pkg, &base, SilentProgress).unwrap();
    let output = dir.path().join("merged");
    let preview = export_preview(&base, &output, true);
    assert!(!output.exists());
    assert_eq!(preview.update_version.as_deref(), Some("01.12"));
    let required = preview.filesystem.required_free_bytes;
    let result = run(
        Path::new(""),
        Operation::Export {
            base: base.clone(),
            preview,
            resume: true,
        },
        &SilentProgress,
    )
    .unwrap();
    assert!(matches!(
        result,
        Outcome::Finished {
            incomplete: false,
            ..
        }
    ));
    assert_eq!(fs::read(output.join("base.txt")).unwrap(), b"UPDATED");
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    let original_manifest = fs::read(output.join(".orbis-export.json")).unwrap();
    let preview = export_preview(&base, &output, true);
    assert!(preview.filesystem.required_free_bytes < required);
    assert_eq!(
        fs::read(output.join(".orbis-export.json")).unwrap(),
        original_manifest
    );
    fs::remove_file(output.join("base.txt")).unwrap();
    run(
        Path::new(""),
        Operation::Export {
            base: base.clone(),
            preview,
            resume: true,
        },
        &SilentProgress,
    )
    .unwrap();
    assert_eq!(fs::read(output.join("base.txt")).unwrap(), b"UPDATED");
    fs::write(output.join("base.txt"), b"USER CONTENT").unwrap();
    assert!(
        run(
            Path::new(""),
            Operation::PreviewExport {
                base,
                output: output.clone(),
                resume: true
            },
            &SilentProgress
        )
        .is_err()
    );
    assert_eq!(fs::read(output.join("base.txt")).unwrap(), b"USER CONTENT");
}

#[test]
fn export_rejects_changed_previews_and_cancellation_never_creates_output() {
    let dir = tempfile::tempdir().unwrap();
    let base = common::base_game(dir.path());
    let output = dir.path().join("merged");
    let preview = export_preview(&base, &output, false);
    fs::write(base.join("added.txt"), b"new file").unwrap();
    let error = run(
        Path::new(""),
        Operation::Export {
            base: base.clone(),
            preview,
            resume: false,
        },
        &SilentProgress,
    )
    .unwrap_err();
    assert!(error.details.contains("changed after the preview"));
    assert!(!output.exists());
    let preview = export_preview(&base, &output, false);
    let progress = Progress::default();
    progress.cancelled.store(true, Ordering::Relaxed);
    assert!(
        run(
            Path::new(""),
            Operation::Export {
                base,
                preview,
                resume: false
            },
            &progress
        )
        .unwrap_err()
        .cancelled
    );
    assert!(!output.exists());
}

#[test]
fn cancellation_during_export_verification_preserves_the_copy_and_reports_cancellation() {
    struct CancelVerification(std::sync::atomic::AtomicBool);
    impl ExtractProgress for CancelVerification {
        fn phase(&self, name: &str) {
            if name == "Verifying exported files" {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let base = common::base_game(dir.path());
    let output = dir.path().join("merged");
    let preview = export_preview(&base, &output, false);
    let error = run(
        Path::new(""),
        Operation::Export {
            base,
            preview,
            resume: false,
        },
        &CancelVerification(Default::default()),
    )
    .unwrap_err();
    assert!(error.cancelled);
    assert!(error.details.contains("Export created"));
    assert!(output.join("base.txt").is_file());
    assert!(
        install::verify_directory(&output, SilentProgress)
            .unwrap()
            .issues
            .is_empty()
    );
}

#[test]
fn automatic_export_verification_reports_corrupt_output_without_changing_sources() {
    struct Corrupt(PathBuf);
    impl ExtractProgress for Corrupt {
        fn phase(&self, name: &str) {
            if name == "Verifying exported files" {
                fs::write(self.0.join("base.txt"), b"FAIL").unwrap();
            }
        }
    }
    for resume in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let base = common::base_game(dir.path());
        let output = dir.path().join("merged");
        let preview = export_preview(&base, &output, resume);
        let Outcome::Finished {
            title, incomplete, ..
        } = run(
            Path::new(""),
            Operation::Export {
                base: base.clone(),
                preview,
                resume,
            },
            &Corrupt(output.clone()),
        )
        .unwrap()
        else {
            panic!()
        };
        assert!(incomplete);
        assert_eq!(title, "Export verification failed");
        assert_eq!(fs::read(output.join("base.txt")).unwrap(), b"FAIL");
        assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    }
}

#[cfg(unix)]
#[test]
fn automatic_export_verification_rejects_a_directory_replaced_while_copying() {
    struct Replace {
        output: PathBuf,
        moved: PathBuf,
        replacement: PathBuf,
    }
    impl ExtractProgress for Replace {
        fn phase(&self, name: &str) {
            if name == "Resuming merged export" {
                fs::rename(&self.output, &self.moved).unwrap();
                fs::rename(&self.replacement, &self.output).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let base = common::base_game(dir.path());
    let progress = Replace {
        output: dir.path().join("merged"),
        moved: dir.path().join("moved"),
        replacement: dir.path().join("replacement"),
    };
    fs::write(base.join("base.txt"), b"EVIL").unwrap();
    install::export_merged(&base, &progress.replacement, SilentProgress).unwrap();
    fs::write(base.join("base.txt"), b"BASE").unwrap();
    let preview = export_preview(&base, &progress.output, true);
    let error = run(
        Path::new(""),
        Operation::Export {
            base: base.clone(),
            preview,
            resume: true,
        },
        &progress,
    )
    .unwrap_err();
    assert!(error.details.contains("Export created"));
    assert!(error.details.contains("export destination changed"));
    assert_eq!(fs::read(progress.moved.join("base.txt")).unwrap(), b"BASE");
    assert_eq!(fs::read(progress.output.join("base.txt")).unwrap(), b"EVIL");
    assert!(
        install::verify_directory(&progress.output, SilentProgress)
            .unwrap()
            .issues
            .is_empty()
    );
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
}

#[test]
fn inspects_then_extracts_real_package_without_a_cli() {
    let dir = tempfile::tempdir().unwrap();
    let path = package(dir.path());
    let Outcome::Inspected(info) = run(&path, Operation::Inspect, &SilentProgress).unwrap() else {
        panic!()
    };
    assert_eq!(info.title_id, "CUSA00000");
    assert_eq!(info.version.as_deref(), Some("01.01"));
    assert_eq!(info.region, Some("North America"));
    assert_eq!(info.content_id, "UP0000-CUSA00000_00-0000000000000000");
    assert_eq!(info.metadata_entries, 3);
    assert_eq!(info.missing_keys, 0);
    assert!(info.required_system.is_none());
    assert!(info.artwork.is_none());
    assert!(info.update_supported);
    let output = dir.path().join("output");
    let result = run(
        &path,
        Operation::Extract {
            output: output.clone(),
            resume: false,
        },
        &SilentProgress,
    )
    .unwrap();
    assert!(matches!(
        result,
        Outcome::Finished {
            incomplete: false,
            ..
        }
    ));
    assert_eq!(fs::read(output.join("new.txt")).unwrap(), b"NEW DATA");
    assert!(output.join("sce_sys/param.sfo").is_file());
}

#[test]
fn resume_reuses_matching_files_and_preserves_conflicting_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = package(dir.path());
    let output = dir.path().join("output");
    let extract = |resume| {
        run(
            &path,
            Operation::Extract {
                output: output.clone(),
                resume,
            },
            &SilentProgress,
        )
    };
    extract(false).unwrap();
    extract(true).unwrap();
    fs::write(output.join("new.txt"), b"keep this").unwrap();
    assert!(extract(false).is_err());
    assert!(extract(true).is_err());
    assert_eq!(fs::read(output.join("new.txt")).unwrap(), b"keep this");
}

#[test]
fn previews_and_installs_updates_without_changing_the_base() {
    let dir = tempfile::tempdir().unwrap();
    let base = common::base_game(dir.path());
    let path = package(dir.path());
    let Outcome::Preview(preview) = run(
        &path,
        Operation::PreviewUpdate {
            base: base.clone(),
            mode: install::InstallMode::Separate,
        },
        &SilentProgress,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(preview.current_version, "01.00");
    assert!(!preview.destination.exists());
    let result = run(
        &path,
        Operation::InstallUpdate {
            base: base.clone(),
            preview: Box::new(preview),
        },
        &SilentProgress,
    )
    .unwrap();
    let Outcome::Finished {
        output,
        incomplete: false,
        ..
    } = result
    else {
        panic!()
    };
    assert_eq!(fs::read(output.join("new.txt")).unwrap(), b"NEW DATA");
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
    assert!(!base.join("new.txt").exists());
}

#[test]
fn refuses_a_changed_update_after_preview() {
    let dir = tempfile::tempdir().unwrap();
    let base = common::base_game(dir.path());
    let path = package(dir.path());
    let Outcome::Preview(preview) = run(
        &path,
        Operation::PreviewUpdate {
            base: base.clone(),
            mode: install::InstallMode::Separate,
        },
        &SilentProgress,
    )
    .unwrap() else {
        panic!()
    };
    fs::write(&path, common::update("01.02", "new.txt", b"NEW DATA")).unwrap();
    let failure = run(
        &path,
        Operation::InstallUpdate {
            base,
            preview: Box::new(preview),
        },
        &SilentProgress,
    )
    .unwrap_err();
    assert!(failure.details.contains("changed after the preview"));
    assert!(!dir.path().join("CUSA00000-UPDATE").exists());
}

#[test]
fn incomplete_extraction_is_not_reported_as_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = package(dir.path());
    let mut bytes = fs::read(&path).unwrap();
    // Encrypt the metadata entry with an unavailable key slot. The inner PFS
    // remains readable, so this must produce a partial result, not full success.
    common::be32(&mut bytes, 0x1048, 0x8000_0000);
    fs::write(&path, bytes).unwrap();
    let Outcome::Inspected(info) = run(&path, Operation::Inspect, &SilentProgress).unwrap() else {
        panic!()
    };
    assert_eq!(info.missing_keys, 1);
    assert!(!info.warnings.is_empty());
    let result = run(
        &path,
        Operation::Extract {
            output: dir.path().join("output"),
            resume: false,
        },
        &SilentProgress,
    )
    .unwrap();
    assert!(matches!(
        result,
        Outcome::Finished {
            incomplete: true,
            ..
        }
    ));
}

#[test]
fn cancellation_creates_no_output_and_is_distinct_from_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = package(dir.path());
    let output = dir.path().join("output");
    let progress = Progress::default();
    progress.cancelled.store(true, Ordering::Relaxed);
    let error = run(
        &path,
        Operation::Extract {
            output: output.clone(),
            resume: false,
        },
        &progress,
    )
    .unwrap_err();
    assert!(error.cancelled);
    assert!(!output.exists());
}

#[test]
fn invalid_inputs_and_unsafe_folder_names_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("other.pkg");
    fs::write(&path, b"not a PS4 package").unwrap();
    assert!(run(&path, Operation::Inspect, &SilentProgress).is_err());
    assert!(run(dir.path(), Operation::Inspect, &SilentProgress).is_err());
    for name in [
        "",
        "../base",
        "/tmp",
        "..",
        "C:\\games",
        "CON",
        "LPT1",
        "a/b",
        "a\\b",
    ] {
        assert!(!folder_name_valid(name));
    }
    assert!(folder_name_valid("CUSA00000-update_01"));
}

#[test]
fn progress_is_bounded_and_tracks_the_library_callback_order() {
    let progress = Progress::default();
    progress.bytes_total(100);
    progress.bytes_processed(80);
    progress.phase("Checking");
    assert_eq!(progress.processed.load(Ordering::Relaxed), 80);
    assert_eq!(progress.total.load(Ordering::Relaxed), 100);
    progress.bytes_total(200);
    assert_eq!(progress.processed.load(Ordering::Relaxed), 0);
    assert_eq!(progress.total.load(Ordering::Relaxed), 200);
    for _ in 0..1_000 {
        progress.warning("Skipped entry");
    }
    let snapshot = progress.snapshot();
    assert_eq!(snapshot.warning_count, 1_000);
    assert_eq!(snapshot.warnings.len(), 100);
}

#[test]
fn inspection_reads_display_metadata_and_optional_artwork() {
    // Independent SFO encoding with strings and a packed firmware integer.
    let fields: [(&str, u16, &[u8]); 5] = [
        ("TITLE", 0x204, b"A friendly application\0"),
        ("CATEGORY", 0x204, b"gde\0"),
        ("APP_VER", 0x204, b"01.23\0"),
        ("VERSION", 0x204, b"01.00\0"),
        ("SYSTEM_VER", 0x404, &0x0950_0000_u32.to_le_bytes()),
    ];
    let mut sfo = vec![0_u8; 20 + 16 * fields.len()];
    sfo[..4].copy_from_slice(b"\0PSF");
    common::le32(&mut sfo, 4, 0x101);
    common::le32(&mut sfo, 8, (20 + 16 * fields.len()) as u32);
    common::le32(&mut sfo, 16, fields.len() as u32);
    let mut values = Vec::new();
    let mut key_offset = 0_u16;
    for (index, (key, kind, value)) in fields.into_iter().enumerate() {
        let entry = 20 + index * 16;
        sfo[entry..entry + 2].copy_from_slice(&key_offset.to_le_bytes());
        sfo[entry + 2..entry + 4].copy_from_slice(&kind.to_le_bytes());
        common::le32(&mut sfo, entry + 4, value.len() as u32);
        common::le32(&mut sfo, entry + 8, value.len().div_ceil(4) as u32 * 4);
        common::le32(&mut sfo, entry + 12, values.len() as u32);
        values.extend_from_slice(value);
        values.resize(values.len().div_ceil(4) * 4, 0);
        sfo.extend(key.bytes().chain([0]));
        key_offset += key.len() as u16 + 1;
    }
    sfo.resize(sfo.len().div_ceil(4) * 4, 0);
    let data_offset = sfo.len() as u32;
    common::le32(&mut sfo, 12, data_offset);
    sfo.extend(values);
    let icon = png(320, 160);
    let mut bytes = common::pkg(common::pfs("hello.txt", b"hello", false));
    assert!(sfo.len() < 0x600);
    bytes[0x1a00..0x1a00 + sfo.len()].copy_from_slice(&sfo);
    common::be32(&mut bytes, 0x1054, sfo.len() as u32);
    common::be32(&mut bytes, 0x10, 4);
    common::be32(&mut bytes, 0x1060, 0x1200); // ICON0.PNG
    let icon_offset = bytes.len() as u32;
    common::be32(&mut bytes, 0x1070, icon_offset);
    common::be32(&mut bytes, 0x1074, icon.len() as u32);
    bytes.extend(icon);
    let size = bytes.len() as u64;
    common::be64(&mut bytes, 0x430, size);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.pkg");
    fs::write(&path, bytes).unwrap();
    let Outcome::Inspected(info) = run(&path, Operation::Inspect, &SilentProgress).unwrap() else {
        panic!()
    };
    assert_eq!(info.title, "A friendly application");
    assert_eq!(info.kind, "Application");
    assert_eq!(info.version.as_deref(), Some("01.23"));
    assert_eq!(info.package_version.as_deref(), Some("01.00"));
    assert_eq!(info.required_system.as_deref(), Some("9.50"));
    assert_eq!(info.artwork.unwrap().size, [160, 80]);
    assert_eq!(content_region("XX"), None);
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut output = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(width, height, image::Rgba([42, 100, 180, 255]))
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}

#[test]
fn optional_artwork_rejects_corrupt_or_excessive_images() {
    assert!(decode_artwork(b"not a PNG").is_none());
    assert!(decode_artwork(&png(1025, 1)).is_none());
    assert!(decode_artwork(&png(1, 1025)).is_none());
    assert!(decode_artwork(&vec![0; 4 * 1024 * 1024 + 1]).is_none());
}

#[test]
fn merge_preview_install_and_restore_work_without_the_original_package() {
    let dir = tempfile::tempdir().unwrap();
    let base = common::base_game(dir.path());
    let original_sfo = fs::read(base.join("sce_sys/param.sfo")).unwrap();
    let path = package(dir.path());
    let Outcome::Preview(preview) = run(
        &path,
        Operation::PreviewUpdate {
            base: base.clone(),
            mode: install::InstallMode::Merge,
        },
        &SilentProgress,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(preview.mode, install::InstallMode::Merge);
    assert_eq!(preview.destination, fs::canonicalize(&base).unwrap());
    let Outcome::Merged { state, output } = run(
        &path,
        Operation::InstallUpdate {
            base: base.clone(),
            preview: Box::new(preview),
        },
        &SilentProgress,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(state.base_version, "01.00");
    assert_eq!(fs::read(output.join("new.txt")).unwrap(), b"NEW DATA");
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        run(
            &path,
            Operation::CheckGame { base: base.clone() },
            &SilentProgress
        )
        .unwrap(),
        Outcome::GameState(_)
    ));
    assert!(matches!(
        run(
            &path,
            Operation::RestoreBase { base: base.clone() },
            &SilentProgress
        )
        .unwrap(),
        Outcome::Finished {
            incomplete: false,
            ..
        }
    ));
    assert_eq!(
        fs::read(base.join("sce_sys/param.sfo")).unwrap(),
        original_sfo
    );
    assert!(!base.join("new.txt").exists());
    assert!(run(&path, Operation::RestoreBase { base }, &SilentProgress).is_err());
}

#[test]
fn checking_an_existing_merge_needs_no_package_and_offers_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let base = common::base_game(dir.path());
    let pkg = orbis_pkg::Pkg::new(common::update("01.01", "new.txt", b"DATA")).unwrap();
    install::install_with_mode(&pkg, &base, install::InstallMode::Merge, SilentProgress).unwrap();
    // A restore marker persisted just before the process stopped.
    fs::write(
        dir.path()
            .join(".CUSA00000.orbis-install/merge/pending.json"),
        b"\"restore\"",
    )
    .unwrap();
    let path = dir.path().join("missing.pkg");
    let Outcome::GameState(state) = run(
        &path,
        Operation::PreviewUpdate {
            base: base.clone(),
            mode: install::InstallMode::Separate,
        },
        &SilentProgress,
    )
    .unwrap() else {
        panic!()
    };
    assert!(state.pending_recovery);
    run(
        &path,
        Operation::RecoverBase { base: base.clone() },
        &SilentProgress,
    )
    .unwrap();
    assert!(!base.join("new.txt").exists());
}
