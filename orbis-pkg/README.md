# orbis-pkg

Rust library for parsing and extracting PlayStation 4 PKG files.

## Features

- Parse PKG headers and entry tables
- Extract PKG metadata (content ID, title ID, content type, flags)
- Decrypt PKG entries
- Parse bounds-checked PARAM.SFO identity fields, typed values and application versions
- Resolve safe metadata filenames for unknown entry IDs
- Derive all seven entry keys from a known passcode with `Pkg::load_passcode`
- Verify stored SHA-256 digests with `Pkg::verify_integrity`; missing digests are counted separately

`sfo::Sfo::parse` retains string, integer, and binary value types.
`sfo::AppMetadata::parse` requires `TITLE_ID`, `CONTENT_ID`, `CATEGORY`, and
`APP_VER`, and checks that the content ID names the same title. Application
versions are compared numerically; the content ID's version field is unrelated.

## Inspecting unsupported packages

`Pkg::parse(bytes)` reads metadata without decrypting keys. Headers, the entry
table, and unencrypted entry payloads remain available. Call `load_entry_keys()`
before reading encrypted entries and `load_pfs_key()` before opening encrypted
filesystem data. The existing `Pkg::new(bytes)` constructor still loads both
keys eagerly and fails if either cannot be loaded.

## Usage

```no_run
use orbis_pkg::Pkg;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
// Open a PKG file
let pkg = Pkg::new(std::fs::read("game.pkg")?)?;

// Access header information
let header = pkg.header();
println!("Content ID: {}", header.content_id());
println!("Title ID: {}", header.content_id().title_id());

// Iterate over entries
for result in pkg.entries() {
    let (index, entry) = result?;
    let data = pkg.entry_data(&entry)?;
    // Process entry data
}

// Access PFS image for game content
if let Some(pfs_image) = pkg.get_pfs_image() {
    println!("PFS image: {} bytes", pfs_image.data.len());
    // Pass data and ekpfs to orbis_pfs::open_slice to access the filesystem.
}
# Ok(())
# }
```

For per-check results, use `verify_integrity_with` and a fallible observer. Events
use the shared `orbis-integrity` types and include a location, status, unavailable
reason, and count. Verification streams events and stops on the first mismatch,
observer error or cancellation; the original `verify_integrity` API remains
available for totals only.
