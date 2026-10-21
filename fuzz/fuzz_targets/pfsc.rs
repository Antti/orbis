#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }
    if let Ok(image) = orbis_pfs::pfsc::PfscImage::open(orbis_fuzz::Slice(data)) {
        orbis_fuzz::read_image(&image);
    }
});
