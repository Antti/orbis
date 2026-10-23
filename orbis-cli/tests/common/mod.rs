use orbis_pfs::image::Image;
use std::io;

pub const BS: usize = 4096;
pub fn le32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
pub fn le64(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_le_bytes());
}
pub fn be32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_be_bytes());
}
pub fn be64(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_be_bytes());
}

#[derive(Clone)]
pub struct Bytes(pub Vec<u8>);
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

pub fn dirent(b: &mut [u8], inode: u32, ty: u32, name: &str) {
    le32(b, 0, inode);
    le32(b, 4, ty);
    le32(b, 8, name.len() as u32);
    le32(b, 12, (16 + name.len()) as u32);
    b[16..16 + name.len()].copy_from_slice(name.as_bytes());
}

// root -> uroot -> named file, using unsigned, contiguous inode block maps.
pub fn pfs(name: &str, contents: &[u8], compressed: bool) -> Vec<u8> {
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
        le64(&mut b, base + 16, len as u64);
        le64(
            &mut b,
            base + 8,
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

pub fn pkg(inner: Vec<u8>) -> Vec<u8> {
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

pub fn pfsc(block_size: u32, original_block_size: u64, len: u64, offsets: &[u64]) -> Bytes {
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

/// Independently encoded SFO, using aligned value slots as real SFO writers do.
#[allow(dead_code)]
pub fn sfo(title: &str, category: &str, version: &str) -> Vec<u8> {
    let content = format!("UP0000-{title}_00-0000000000000000");
    let fields = [
        ("TITLE_ID", title),
        ("CONTENT_ID", content.as_str()),
        ("CATEGORY", category),
        ("APP_VER", version),
    ];
    let mut keys = Vec::new();
    let mut values = Vec::new();
    let mut entries = Vec::new();
    for (key, value) in fields {
        let key_offset = keys.len() as u16;
        keys.extend(key.bytes().chain([0]));
        let value_offset = values.len() as u32;
        let len = value.len() as u32 + 1;
        let capacity = len.div_ceil(4) * 4;
        values.extend(value.bytes().chain([0]));
        values.resize(value_offset as usize + capacity as usize, 0);
        entries.extend(key_offset.to_le_bytes());
        entries.extend(0x204u16.to_le_bytes());
        entries.extend(len.to_le_bytes());
        entries.extend(capacity.to_le_bytes());
        entries.extend(value_offset.to_le_bytes());
    }
    let key_offset = 20 + entries.len();
    let data_offset = (key_offset + keys.len()).div_ceil(4) * 4;
    let mut bytes = vec![0; 20];
    bytes[..4].copy_from_slice(b"\0PSF");
    le32(&mut bytes, 4, 0x101);
    le32(&mut bytes, 8, key_offset as u32);
    le32(&mut bytes, 12, data_offset as u32);
    le32(&mut bytes, 16, 4);
    bytes.extend(entries);
    bytes.extend(keys);
    bytes.resize(data_offset, 0);
    bytes.extend(values);
    bytes
}

#[allow(dead_code)]
pub fn update(version: &str, name: &str, contents: &[u8]) -> Vec<u8> {
    let mut bytes = pkg(pfs(name, contents, false));
    let metadata = sfo("CUSA00000", "gp", version);
    assert!(metadata.len() <= 0x600);
    bytes[0x1a00..0x1a00 + metadata.len()].copy_from_slice(&metadata);
    be32(&mut bytes, 0x1054, metadata.len() as u32);
    be32(&mut bytes, 0x74, 0x1a);
    be32(&mut bytes, 0x78, 0x60000000);
    bytes
}

#[allow(dead_code)]
pub fn base_game(root: &std::path::Path) -> std::path::PathBuf {
    let base = root.join("CUSA00000");
    std::fs::create_dir_all(base.join("sce_sys")).unwrap();
    std::fs::write(
        base.join("sce_sys/param.sfo"),
        sfo("CUSA00000", "gd", "01.00"),
    )
    .unwrap();
    std::fs::write(base.join("base.txt"), b"BASE").unwrap();
    base
}
