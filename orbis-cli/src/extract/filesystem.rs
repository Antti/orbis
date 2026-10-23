use super::{ExtractError, output::valid_name};
use orbis_pfs::directory::DirEntry;
use orbis_pkg::Pkg;
use std::{collections::HashSet, path::PathBuf};

pub(super) struct Filesystem<'a> {
    pub dirs: Vec<PathBuf>,
    pub files: Vec<FileWork<'a>>,
}

pub(super) fn open_filesystems<R: AsRef<[u8]>>(
    pkg: &Pkg<R>,
) -> Result<
    (
        std::sync::Arc<orbis_pfs::Pfs<'_>>,
        std::sync::Arc<orbis_pfs::Pfs<'_>>,
    ),
    ExtractError,
> {
    let outer = open_outer_filesystem(pkg)?;
    let inner = open_inner_filesystem(&outer)?;
    Ok((outer, inner))
}

pub(super) fn open_outer_filesystem<R: AsRef<[u8]>>(
    pkg: &Pkg<R>,
) -> Result<std::sync::Arc<orbis_pfs::Pfs<'_>>, ExtractError> {
    let pfs_image = pkg.get_pfs_image().ok_or(ExtractError::NoPfsImage)?;
    let key = (!pfs_image.ekpfs.is_empty()).then_some(pfs_image.ekpfs);
    orbis_pfs::open_slice(pfs_image.data, key)
        .map_err(|source| ExtractError::OpenOuterPfsFailed { source })
}

pub(super) fn open_inner_filesystem<'a>(
    outer_pfs: &std::sync::Arc<orbis_pfs::Pfs<'a>>,
) -> Result<std::sync::Arc<orbis_pfs::Pfs<'a>>, ExtractError> {
    let mut outer_root = outer_pfs
        .root()
        .open()
        .map_err(|e| ExtractError::OpenOuterSuperRootFailed { source: e })?;

    // Open outer uroot directory.
    let mut outer_uroot = match outer_root.remove(b"uroot") {
        Some(DirEntry::Directory(d)) => d
            .open()
            .map_err(|e| ExtractError::OpenOuterUrootFailed { source: e })?,
        _ => return Err(ExtractError::NoOuterUroot),
    };

    // Get inner PFS image (pfs_image.dat).
    let inner_file = match outer_uroot.remove(b"pfs_image.dat") {
        Some(DirEntry::File(f)) => f,
        _ => return Err(ExtractError::NoInnerImage),
    };

    // Convert the file handle to an Image adapter, then open the inner PFS.
    let is_compressed = inner_file.is_compressed();
    let file_image = inner_file.into_image();

    let inner_pfs = if is_compressed {
        let pfsc = orbis_pfs::pfsc::PfscImage::open(file_image)
            .map_err(|e| ExtractError::CreateDecompressorFailed { source: e })?;
        orbis_pfs::open_image(pfsc).map_err(|e| ExtractError::OpenInnerPfsFailed { source: e })?
    } else {
        orbis_pfs::open_image(file_image)
            .map_err(|e| ExtractError::OpenInnerPfsFailed { source: e })?
    };

    Ok(inner_pfs)
}

pub(super) fn read_filesystem<R: AsRef<[u8]>>(
    pkg: &Pkg<R>,
) -> Result<Filesystem<'_>, ExtractError> {
    let (_, inner_pfs) = open_filesystems(pkg)?;
    let mut inner_root = inner_pfs
        .root()
        .open()
        .map_err(|e| ExtractError::OpenInnerSuperRootFailed { source: e })?;

    // Get inner uroot.
    let inner_uroot = match inner_root.remove(b"uroot") {
        Some(DirEntry::Directory(d)) => d,
        _ => return Err(ExtractError::NoInnerUroot),
    };

    let mut dirs = Vec::new();
    let mut files = Vec::new();
    collect_pfs_items(inner_uroot, &mut dirs, &mut files)?;
    dirs.sort();
    files.sort_by(|a, b| a.output_path.cmp(&b.output_path));
    Ok(Filesystem { dirs, files })
}

/// A file to be extracted, collected during the directory walk.
pub(super) struct FileWork<'a> {
    pub file: orbis_pfs::file::File<'a>,
    pub output_path: PathBuf,
    pub pfs_path: String,
}

/// Walks a PFS directory tree and collects all directories
/// and files into flat lists for later parallel extraction.
fn collect_pfs_items<'a>(
    dir: orbis_pfs::directory::Directory<'a>,
    dirs: &mut Vec<PathBuf>,
    files: &mut Vec<FileWork<'a>>,
) -> Result<(), ExtractError> {
    let mut pending = vec![(dir, PathBuf::new(), String::from("/"))];
    let mut visited = HashSet::new();
    while let Some((dir, output, pfs_path)) = pending.pop() {
        if !visited.insert(dir.inode_number()) {
            return Err(ExtractError::RepeatedDirectory { path: pfs_path });
        }
        let items = dir
            .open()
            .map_err(|e| ExtractError::OpenPfsDirectoryFailed {
                path: pfs_path.to_string(),
                source: e,
            })?;

        for (name, item) in items {
            let name_str =
                std::str::from_utf8(&name).map_err(|_| ExtractError::UnsupportedFileName {
                    path: format!("{}{}", pfs_path, String::from_utf8_lossy(&name)),
                })?;
            if !valid_name(name_str) {
                return Err(ExtractError::UnsupportedFileName {
                    path: format!("{}{}", pfs_path, name_str),
                });
            }

            let item_output = output.join(name_str);
            let item_pfs_path = format!("{}{}", pfs_path, name_str);

            match item {
                DirEntry::Directory(subdir) => {
                    dirs.push(item_output.clone());
                    pending.push((subdir, item_output, format!("{item_pfs_path}/")));
                }
                DirEntry::File(file) => {
                    files.push(FileWork {
                        file,
                        output_path: item_output,
                        pfs_path: item_pfs_path,
                    });
                }
                _ => unreachable!(),
            }
        }
    }

    Ok(())
}
