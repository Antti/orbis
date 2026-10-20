#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }
    if let Ok(pfs) = orbis_pfs::open_slice(data, Some(&[0x43; 32])) {
        orbis_fuzz::walk(pfs);
    }
    if let Ok(pfs) = orbis_pfs::open_image(orbis_fuzz::Slice(data)) {
        orbis_fuzz::walk(pfs);
    }
});
