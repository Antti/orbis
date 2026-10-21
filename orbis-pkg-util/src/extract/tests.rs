use super::*;
use crate::SilentProgress;
use crate::test_support::*;
use orbis_pfs::pfsc::PfscImage;

#[test]
fn extracts_complete_package_and_honors_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("new");
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    let extractor = PkgExtractor::new(&package, SilentProgress, false);
    extractor.extract(&out).unwrap();
    assert_eq!(std::fs::read(out.join("hello.txt")).unwrap(), b"hello");
    assert_eq!(
        std::fs::read(out.join("sce_sys/param.sfo")).unwrap(),
        b"NEW"
    );
    std::fs::write(out.join("sce_sys/param.sfo"), b"KEEP").unwrap();
    assert!(extractor.extract(&out).is_err());
    assert_eq!(
        std::fs::read(out.join("sce_sys/param.sfo")).unwrap(),
        b"KEEP"
    );
    PkgExtractor::new(&package, SilentProgress, true)
        .extract(&out)
        .unwrap();
    assert_eq!(
        std::fs::read(out.join("sce_sys/param.sfo")).unwrap(),
        b"NEW"
    );
}

#[test]
fn standalone_extractors_create_the_output_root() {
    let dir = tempfile::tempdir().unwrap();
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    let extractor = PkgExtractor::new(&package, SilentProgress, false);
    extractor.extract_pfs(dir.path().join("pfs")).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("pfs/hello.txt")).unwrap(),
        b"hello"
    );
    extractor
        .extract_entries(dir.path().join("entries"))
        .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("entries/param.sfo")).unwrap(),
        b"NEW"
    );
}

#[test]
fn pfs_files_respect_overwrite_policy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), b"KEEP").unwrap();
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    assert!(
        PkgExtractor::new(&package, SilentProgress, false)
            .extract_pfs(dir.path())
            .is_err()
    );
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"KEEP"
    );
    PkgExtractor::new(&package, SilentProgress, true)
        .extract_pfs(dir.path())
        .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"hello"
    );
}

#[test]
fn rejects_archive_path_traversal_on_all_platforms() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("output");
    let absolute = dir.path().join("absolute").to_string_lossy().into_owned();
    for name in [
        "../escaped.txt",
        "..",
        ".",
        "",
        "/absolute",
        "sub/file",
        "sub\\file",
        "C:\\escape",
        "file:stream",
        &absolute,
    ] {
        let package = Pkg::new(pkg(pfs(name, b"bad", false))).unwrap();
        assert!(
            matches!(
                PkgExtractor::new(&package, SilentProgress, true).extract_pfs(&out),
                Err(ExtractError::UnsupportedFileName { .. })
            ),
            "accepted {name:?}"
        );
    }
    assert!(!dir.path().join("escaped.txt").exists());
    assert!(!dir.path().join("absolute").exists());
}

#[test]
fn rejects_directory_cycles() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = pfs("loop", b"", false);
    dirent(&mut inner[3 * BS..], 1, 3, "loop");
    let package = Pkg::new(pkg(inner)).unwrap();
    assert!(matches!(
        PkgExtractor::new(&package, SilentProgress, false).extract_pfs(dir.path()),
        Err(ExtractError::RepeatedDirectory { .. })
    ));
}

#[test]
fn decompresses_leaf_files_including_zlib_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let expected = vec![b'A'; BS];
    encoder.write_all(&expected).unwrap();
    let encoded = encoder.finish().unwrap();
    let mut container = pfsc(
        BS as u32,
        BS as u64,
        BS as u64,
        &[80, 80 + encoded.len() as u64],
    )
    .0;
    container[80..80 + encoded.len()].copy_from_slice(&encoded);
    container.truncate(80 + encoded.len());
    let image = PfscImage::open(Bytes(container.clone())).unwrap();
    let mut check = vec![0; BS];
    image.read_exact_at(0, &mut check).unwrap();
    assert_eq!(check, expected);
    let package = Pkg::new(pkg(pfs("compressed.bin", &container, true))).unwrap();
    PkgExtractor::new(&package, SilentProgress, false)
        .extract_pfs(dir.path())
        .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("compressed.bin")).unwrap(),
        expected
    );
}

#[test]
fn failed_decompression_preserves_existing_files_and_removes_staging_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut container = pfsc(BS as u32, BS as u64, BS as u64, &[80, 83]).0;
    container.truncate(83);
    container[80..].copy_from_slice(b"bad");
    let package = Pkg::new(pkg(pfs("hello.txt", &container, true))).unwrap();

    for overwrite in [false, true] {
        let out = dir.path().join(if overwrite { "replace" } else { "new" });
        std::fs::create_dir(&out).unwrap();
        if overwrite {
            std::fs::write(out.join("hello.txt"), b"KEEP").unwrap();
        }
        assert!(
            PkgExtractor::new(&package, SilentProgress, overwrite)
                .extract_pfs(&out)
                .is_err()
        );
        let names: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        if overwrite {
            assert_eq!(std::fs::read(out.join("hello.txt")).unwrap(), b"KEEP");
            assert_eq!(names, ["hello.txt"]);
        } else {
            assert!(names.is_empty());
        }
    }
}

#[test]
fn destination_created_during_staging_is_not_overwritten() {
    for policy in [ExistingFilePolicy::Refuse, ExistingFilePolicy::Verify] {
        let dir = tempfile::tempdir().unwrap();
        let output = output::Output::new(dir.path()).unwrap();
        let mut staged = output.create_file(Path::new("hello.txt"), policy).unwrap();
        staged.write_all(b"NEW").unwrap();
        std::fs::write(dir.path().join("hello.txt"), b"KEEP").unwrap();
        assert!(staged.commit(&SilentProgress).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("hello.txt")).unwrap(),
            b"KEEP"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn extracts_to_a_configured_external_filesystem() {
    // Optional real-volume regression. Always create a private subtree; never
    // overwrite or remove existing content on the supplied volume.
    let Some(root) = std::env::var_os("ORBIS_TEST_EXTRACT_ROOT") else {
        return;
    };
    let temp = tempfile::tempdir_in(root).unwrap();
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    let extractor = PkgExtractor::new(&package, SilentProgress, false);
    let report = extractor
        .extract_with_options(temp.path(), &ExtractOptions::default())
        .unwrap();
    assert_eq!(report.files, 4);
    assert_eq!(
        std::fs::read(temp.path().join("hello.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read(temp.path().join("sce_sys/param.sfo")).unwrap(),
        b"NEW"
    );
    assert!(extractor.extract(temp.path()).is_err());
    std::fs::remove_file(temp.path().join("hello.txt")).unwrap();
    PkgExtractor::new(&package, SilentProgress, false)
        .with_resume(true)
        .extract(temp.path())
        .unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("hello.txt")).unwrap(),
        b"hello"
    );
}

#[test]
fn selection_and_listing_use_extraction_relative_paths() {
    let dir = tempfile::tempdir().unwrap();
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    let extractor = PkgExtractor::new(&package, SilentProgress, false);
    assert!(
        extractor
            .list_entries()
            .unwrap()
            .iter()
            .any(|item| item.path.as_deref() == Some("sce_sys/param.sfo"))
    );
    assert_eq!(
        extractor.list_files().unwrap()[0].path.as_deref(),
        Some("hello.txt")
    );
    let options = ExtractOptions {
        paths: vec!["hello.txt".into()],
        ..Default::default()
    };
    let report = extractor
        .extract_with_options(dir.path(), &options)
        .unwrap();
    assert_eq!(report.files, 1);
    assert_eq!(report.bytes, 5);
    assert!(!dir.path().join("sce_sys").exists());
    let options = ExtractOptions {
        paths: vec!["sce_sys/".into()],
        ..Default::default()
    };
    let out = dir.path().join("metadata");
    assert_eq!(
        extractor
            .extract_with_options(&out, &options)
            .unwrap()
            .files,
        3
    );
    assert!(!out.join("hello.txt").exists());
    for path in ["missing", "../hello.txt", "/hello.txt", "C:\\escape", ""] {
        let out = dir.path().join("invalid");
        let options = ExtractOptions {
            paths: vec![path.into()],
            ..Default::default()
        };
        assert!(
            extractor.extract_with_options(&out, &options).is_err(),
            "{path}"
        );
        assert!(!out.exists());
    }
}

#[test]
fn skipped_entries_are_reported_as_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = pkg(pfs("hello.txt", b"hello", false));
    be32(&mut bytes, 0x1048, 0x80000000);
    be32(&mut bytes, 0x104c, 0x1000);
    let package = Pkg::new(bytes).unwrap();
    let extractor = PkgExtractor::new(&package, SilentProgress, false);
    let report = extractor
        .extract_with_options(dir.path().join("report"), &ExtractOptions::default())
        .unwrap();
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].path, "sce_sys/param.sfo");
    assert!(report.skipped[0].reason.contains("slot 1"));
    assert!(matches!(
        extractor.extract(dir.path().join("legacy")),
        Err(ExtractError::Incomplete { skipped: 1 })
    ));
    assert_eq!(
        extractor
            .verify(&ExtractOptions::default())
            .unwrap()
            .skipped
            .len(),
        1
    );
}

#[test]
fn metadata_only_does_not_need_a_filesystem_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = pkg(pfs("hello.txt", b"hello", false));
    bytes[0x1900..0x1a00].fill(0);
    let package = Pkg::parse(bytes).unwrap();
    let extractor = PkgExtractor::new(&package, SilentProgress, false);
    let options = ExtractOptions {
        metadata_only: true,
        paths: vec!["sce_sys/param.sfo".into()],
    };
    let report = extractor
        .extract_with_options(dir.path(), &options)
        .unwrap();
    assert_eq!(report.files, 1);
    assert_eq!(
        std::fs::read(dir.path().join("sce_sys/param.sfo")).unwrap(),
        b"NEW"
    );
    assert_eq!(extractor.verify(&options).unwrap().files, 1);
}

#[test]
fn conflicts_are_checked_before_writing_metadata() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), b"KEEP").unwrap();
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    assert!(
        PkgExtractor::new(&package, SilentProgress, false)
            .extract(dir.path())
            .is_err()
    );
    assert!(!dir.path().join("sce_sys").exists());
}

#[test]
fn verification_reads_compressed_data_without_an_output_directory() {
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    let report = PkgExtractor::new(&package, SilentProgress, false)
        .verify(&ExtractOptions::default())
        .unwrap();
    assert_eq!(report.files, 4);
    assert_eq!(report.bytes, 2048 + 256 + 3 + 5);
    let mut corrupt = pfsc(BS as u32, BS as u64, BS as u64, &[80, 83]).0;
    corrupt.truncate(83);
    corrupt[80..].copy_from_slice(b"bad");
    let package = Pkg::new(pkg(pfs("compressed.bin", &corrupt, true))).unwrap();
    assert!(matches!(
        PkgExtractor::new(&package, SilentProgress, false).verify(&ExtractOptions::default()),
        Err(ExtractError::ReadPfsFileFailed { .. })
    ));
}

#[test]
fn cancellation_between_chunks_preserves_the_destination() {
    use std::sync::atomic::{AtomicU64, Ordering};
    struct CancelAfterChunk(AtomicU64);
    impl ExtractProgress for CancelAfterChunk {
        fn bytes_processed(&self, bytes: u64) {
            self.0.fetch_add(bytes, Ordering::Relaxed);
        }
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed) > 0
        }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), b"KEEP").unwrap();
    let package = Pkg::new(pkg(pfs("hello.txt", &vec![b'A'; 16 * 1024 * 1024], false))).unwrap();
    let progress = CancelAfterChunk(AtomicU64::new(0));
    assert!(matches!(
        PkgExtractor::new(&package, progress, true).extract_pfs(dir.path()),
        Err(ExtractError::Cancelled)
    ));
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"KEEP"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn empty_filesystem_still_creates_output() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("empty");
    let mut inner = pfs("ignored", b"", false);
    inner[3 * BS..4 * BS].fill(0);
    let package = Pkg::new(pkg(inner)).unwrap();
    PkgExtractor::new(&package, SilentProgress, false)
        .extract_pfs(&out)
        .unwrap();
    assert!(out.is_dir());
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn refuses_existing_symlinks_outside_output() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("output");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"KEEP").unwrap();
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    symlink(outside.join("keep"), out.join("hello.txt")).unwrap();
    assert!(
        PkgExtractor::new(&package, SilentProgress, true)
            .extract_pfs(&out)
            .is_err()
    );
    assert_eq!(std::fs::read(outside.join("keep")).unwrap(), b"KEEP");
    symlink(&outside, out.join("sce_sys")).unwrap();
    assert!(
        PkgExtractor::new(&package, SilentProgress, true)
            .extract(&out)
            .is_err()
    );
    assert!(!outside.join("param.sfo").exists());
}

#[test]
fn resume_checks_existing_contents_and_copies_only_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    let extractor = PkgExtractor::new(&package, SilentProgress, false).with_resume(true);
    std::fs::create_dir(dir.path().join("sce_sys")).unwrap();
    std::fs::write(dir.path().join("sce_sys/param.sfo"), b"NEW").unwrap();
    let before = std::fs::metadata(dir.path().join("sce_sys/param.sfo"))
        .unwrap()
        .modified()
        .unwrap();
    extractor.extract(dir.path()).unwrap();
    assert_eq!(
        std::fs::metadata(dir.path().join("sce_sys/param.sfo"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"hello"
    );
    for bytes in [b"wrong".as_slice(), b"hell", b"hello extra"] {
        std::fs::write(dir.path().join("hello.txt"), bytes).unwrap();
        assert!(extractor.extract(dir.path()).is_err());
        assert_eq!(std::fs::read(dir.path().join("hello.txt")).unwrap(), bytes);
    }
}

#[test]
fn resume_precedes_overwrite_and_disabling_it_restores_the_builder_policy() {
    let dir = tempfile::tempdir().unwrap();
    let package = Pkg::new(pkg(pfs("hello.txt", b"hello", false))).unwrap();
    let extractor = PkgExtractor::new(&package, SilentProgress, true).with_resume(true);
    extractor.extract(dir.path()).unwrap();
    std::fs::write(dir.path().join("hello.txt"), b"EDITED").unwrap();
    assert!(extractor.extract(dir.path()).is_err());
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"EDITED"
    );
    extractor.with_resume(false).extract(dir.path()).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"hello"
    );
    assert!(
        PkgExtractor::new(&package, SilentProgress, false)
            .with_resume(true)
            .with_resume(false)
            .extract(dir.path())
            .is_err()
    );
}
