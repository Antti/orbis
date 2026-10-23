use super::*;
use crate::worker::Progress;
use orbis_cli::{ExtractProgress, SilentProgress};
use std::{fs, sync::atomic::Ordering};

#[allow(dead_code)]
#[path = "../../../orbis-cli/tests/common/mod.rs"]
mod common;

fn package(root: &Path) -> PathBuf {
    let path = root.join("game.pkg");
    fs::write(&path, common::update("01.01", "new.txt", b"NEW DATA")).unwrap();
    path
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
