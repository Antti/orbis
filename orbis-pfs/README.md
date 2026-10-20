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
