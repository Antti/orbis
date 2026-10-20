use super::*;
use crate::SilentProgress;
use orbis_pfs::pfsc::PfscImage;
use std::io;

const BS: usize = 4096;
fn le32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn le64(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_le_bytes());
}
fn be32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_be_bytes());
}
fn be64(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_be_bytes());
}

#[derive(Clone)]
struct Bytes(Vec<u8>);
impl Image for Bytes {
    fn read_at(&self, off: u64, out: &mut [u8]) -> io::Result<usize> {
        let src = self.0.get(off as usize..).unwrap_or_default();
        let n = src.len().min(out.len());
        out[..n].copy_from_slice(&src[..n]);
        Ok(n)
    }
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
}

fn dirent(b: &mut [u8], inode: u32, ty: u32, name: &str) {
    le32(b, 0, inode);
    le32(b, 4, ty);
    le32(b, 8, name.len() as u32);
    le32(b, 12, (16 + name.len()) as u32);
    b[16..16 + name.len()].copy_from_slice(name.as_bytes());
}

// root -> uroot -> named file, using unsigned, contiguous inode block maps.
fn pfs(name: &str, contents: &[u8], compressed: bool) -> Vec<u8> {
    let count = contents.len().div_ceil(BS);
    let mut b = vec![0; (4 + count) * BS];
    le64(&mut b, 0, 1);
    le64(&mut b, 8, 20130315);
    le32(&mut b, 0x20, BS as u32);
    le64(&mut b, 0x28, (4 + count) as u64);
    le64(&mut b, 0x30, 3);
    le64(&mut b, 0x40, 1);
    for i in 0..3 {
        let base = BS + i * 168;
        b[base..base + 2].copy_from_slice(&(if i == 2 { 0x8000u16 } else { 0x4000 }).to_le_bytes());
        le32(&mut b, base + 4, if i == 2 && compressed { 1 } else { 0 });
        let len = if i == 2 && compressed {
            u64::from_le_bytes(contents[40..48].try_into().unwrap()) as usize
        } else if i == 2 {
            contents.len()
        } else {
            BS
        };
        le64(&mut b, base + 8, len as u64);
        le64(
            &mut b,
            base + 16,
            if i == 2 {
                contents.len() as u64
            } else {
                len as u64
            },
        );
        le32(&mut b, base + 96, if i == 2 { count as u32 } else { 1 });
        le32(&mut b, base + 100, (2 + i) as u32);
        le32(&mut b, base + 104, u32::MAX);
    }
    dirent(&mut b[2 * BS..], 1, 3, "uroot");
    dirent(&mut b[3 * BS..], 2, 2, name);
    b[4 * BS..4 * BS + contents.len()].copy_from_slice(contents);
    b
}

fn pkg(inner: Vec<u8>) -> Vec<u8> {
    let outer = pfs("pfs_image.dat", &inner, false);
    let mut b = vec![0; 0x2000 + outer.len()];
    be32(&mut b, 0, 0x7F434E54);
    be32(&mut b, 0x10, 3);
    be32(&mut b, 0x18, 0x1000);
    b[0x40..0x64].copy_from_slice(b"UP0000-CUSA00000_00-0000000000000000");
    be64(&mut b, 0x410, 0x2000);
    be64(&mut b, 0x418, outer.len() as u64);
    let len = b.len();
    be64(&mut b, 0x430, len as u64);
    for (i, (id, offset, size)) in [
        (0x10, 0x1100, 2048),
        (0x20, 0x1900, 256),
        (0x1000, 0x1a00, 3),
    ]
    .into_iter()
    .enumerate()
    {
        let base = 0x1000 + i * 32;
        be32(&mut b, base, id);
        be32(&mut b, base + 16, offset);
        be32(&mut b, base + 20, size);
    }
    let key = rsa::RsaPublicKey::from(orbis_pkg::keys::pkg_key3());
    let cipher = key
        .encrypt(&mut rand::rngs::OsRng, rsa::Pkcs1v15Encrypt, &[0x42; 32])
        .unwrap();
    let key3 = 0x1100 + 256 + 3 * 256;
    b[key3..key3 + 256].copy_from_slice(&cipher);
    let key = rsa::RsaPublicKey::from(orbis_pkg::keys::fake_pfs_key());
    let cipher = key
        .encrypt(&mut rand::rngs::OsRng, rsa::Pkcs1v15Encrypt, &[0x43; 32])
        .unwrap();
    b[0x1900..0x1a00].copy_from_slice(&cipher);
    b[0x1a00..0x1a03].copy_from_slice(b"NEW");
    b[0x2000..].copy_from_slice(&outer);
    b
}

fn pfsc(block_size: u32, original_block_size: u64, len: u64, offsets: &[u64]) -> Bytes {
    let mut b = vec![0; 80 + 2 * BS];
    b[..4].copy_from_slice(b"PFSC");
    le32(&mut b, 12, block_size);
    le64(&mut b, 16, original_block_size);
    le64(&mut b, 24, 48);
    le64(&mut b, 40, len);
    for (i, off) in offsets.iter().enumerate() {
        le64(&mut b, 48 + i * 8, *off);
    }
    b[80..].fill(0x41);
    Bytes(b)
}

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
