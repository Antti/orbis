# orbis-pfs

Rust library for reading PFS (PlayStation File System) images used in PS4 games.

## Features

- Parse PFS headers and inodes
- Read files and directories from PFS images
- XTS-AES decryption support
- PFSC (compressed PFS) decompression

## Usage

```no_run
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let pfs_data = std::fs::read("image.pfs")?;
// Pass Some(ekpfs_key) instead of None for an encrypted image.
let pfs = orbis_pfs::open_slice(&pfs_data, None)?;

// Access the root directory
let root = pfs.root().open()?;

// Iterate over directory entries
for (name, item) in root {
    match item {
        orbis_pfs::directory::DirEntry::File(file) => {
            println!("{}: {} bytes", String::from_utf8_lossy(&name), file.len());
        }
        orbis_pfs::directory::DirEntry::Directory(dir) => {
            println!("{}: {} entries", String::from_utf8_lossy(&name), dir.open()?.len());
        }
        _ => {}
    }
}
# Ok(())
# }
```

## Stored integrity checks

`Pfs::verify_integrity(Some(ekpfs), || false)` checks stored HMACs over decrypted
signed header, inode-table and data blocks. It walks direct and supported indirect
block references and returns counts of verified and unavailable checks. Supply a
cancellation callback for long-running checks. Unsigned images have no HMACs;
missing keys or zero signatures never count as verified. This does not verify
publisher authenticity. See the [CLI integrity documentation](../orbis-cli/README.md#check-data-without-extracting-it)
for supported layouts and incomplete-result handling.

For per-check results, use `verify_integrity_with` and a fallible observer. Events
use the shared `orbis-integrity` types and include a location, status, unavailable
reason, and count. Verification streams events and stops on the first mismatch,
observer error or cancellation; the original `verify_integrity` API remains
available for totals only.
