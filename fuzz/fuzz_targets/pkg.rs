#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::Component;

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }
    if let Ok(mut pkg) = orbis_pkg::Pkg::parse(data) {
        let _ = pkg.header().content_id().to_string();
        let _ = pkg.get_pfs_image();
        let _ = pkg.load_passcode(&[b'0'; 32]);
        let _ = pkg.verify_integrity(true, orbis_fuzz::budget(512));
        for item in pkg.entries().take(1024) {
            let Ok((_, entry)) = item else {
                break;
            };
            if let Ok(path) = pkg.entry_path(&entry) {
                assert!(!path.is_absolute());
                assert!(path.components().all(|c| matches!(c, Component::Normal(_))));
            }
            if let Ok(bytes) = pkg.entry_data(&entry) {
                assert_eq!(bytes.len(), entry.data_size());
                if entry.entry_id() == orbis_pkg::entry::EntryId::ParamSfo {
                    let _ = orbis_pkg::sfo::Sfo::parse(&bytes);
                }
            }
        }
    }
});
