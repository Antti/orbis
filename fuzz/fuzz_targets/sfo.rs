#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(sfo) = orbis_pkg::sfo::Sfo::parse(data) {
        for (name, value) in sfo.entries() {
            assert!(!name.is_empty());
            assert_eq!(sfo.get(name), Some(value));
        }
        let _ = orbis_pkg::sfo::AppMetadata::parse(data);
    }
});
