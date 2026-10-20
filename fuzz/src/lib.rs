//! Bounded traversal for fuzzing. Parser input bytes are never repaired or
//! filtered by header fields. Limits only cap downstream work per input.
use orbis_pfs::{directory::DirEntry, image::Image};
use std::{cell::Cell, collections::HashSet};

pub fn budget(limit: usize) -> impl Fn() -> bool {
    let remaining = Cell::new(limit);
    move || {
        let n = remaining.get();
        remaining.set(n.saturating_sub(1));
        n == 0
    }
}

pub fn read_image(image: &impl Image) {
    let mut buffer = [0; 4096];
    for offset in [
        0,
        1,
        4095,
        image.len() / 2,
        image.len().saturating_sub(1),
        image.len(),
        u64::MAX,
    ] {
        if let Ok(n) = image.read_at(offset, &mut buffer) {
            assert!(n <= buffer.len());
            if offset >= image.len() {
                assert_eq!(n, 0);
            }
        }
    }
}

pub fn walk(pfs: std::sync::Arc<orbis_pfs::Pfs<'_>>) {
    let _ = pfs.verify_integrity(Some(&[0x43; 32]), budget(512));
    let mut pending = vec![pfs.root()];
    let mut visited = HashSet::new();
    let mut items = 0;
    while let Some(dir) = pending.pop() {
        if items >= 128 {
            break;
        }
        if !visited.insert(dir.inode_number()) {
            continue;
        }
        let Ok(entries) = dir.open() else {
            continue;
        };
        for (_, entry) in entries {
            items += 1;
            if items >= 128 {
                break;
            }
            match entry {
                DirEntry::Directory(dir) => pending.push(dir),
                DirEntry::File(file) => {
                    let compressed = file.is_compressed();
                    let image = file.into_image();
                    read_image(&image);
                    if compressed && let Ok(image) = orbis_pfs::pfsc::PfscImage::open(image) {
                        read_image(&image);
                    }
                }
                _ => {}
            }
        }
    }
}

pub struct Slice<'a>(pub &'a [u8]);
impl Image for Slice<'_> {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, output: &mut [u8]) -> std::io::Result<usize> {
        let source = usize::try_from(offset)
            .ok()
            .and_then(|at| self.0.get(at..))
            .unwrap_or_default();
        let count = source.len().min(output.len());
        output[..count].copy_from_slice(&source[..count]);
        Ok(count)
    }
}
