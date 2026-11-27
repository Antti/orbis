use orbis_pfs::header::{Mode, read_mode};
use orbis_pkg::Pkg;
use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum PackageFilesystemError {
    #[snafu(display("The package filesystem image is missing or incomplete."))]
    MissingImage,
    #[snafu(display("Cannot read the package filesystem image header."))]
    Header {
        source: orbis_pfs::header::ReadError,
    },
    #[snafu(display("Cannot read the key for the encrypted package filesystem."))]
    ImageKey { source: orbis_pkg::OpenError },
}

/// Checks the on-disk prerequisites that can be inspected before entering a passcode.
pub(crate) fn inspect<R: AsRef<[u8]>>(pkg: &Pkg<R>) -> Result<Mode, PackageFilesystemError> {
    let image = pkg
        .get_pfs_image()
        .ok_or(PackageFilesystemError::MissingImage)?;
    let mode = read_mode(image.data).context(HeaderSnafu)?;
    if mode.is_encrypted() {
        let _ = pkg.pfs_image_key_entry().context(ImageKeySnafu)?;
    }
    Ok(mode)
}

/// Loads the filesystem key when required by the outer PFS header.
pub fn prepare_filesystem<R: AsRef<[u8]>>(pkg: &mut Pkg<R>) -> Result<(), PackageFilesystemError> {
    if inspect(pkg)?.is_encrypted() {
        // A plaintext image-key entry is usable even if slot 3 is unavailable.
        let _ = pkg.load_entry_keys();
        pkg.load_pfs_key().context(ImageKeySnafu)?;
    }
    Ok(())
}
