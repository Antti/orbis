mod error;
mod filesystem;
mod output;

#[cfg(test)]
mod tests;

pub use self::error::ExtractError;
use self::filesystem::{FileWork, Filesystem, read_filesystem};
use self::output::{Output, valid_name};
use crate::progress::ExtractProgress;
use orbis_pfs::image::Image;
use orbis_pkg::{Pkg, entry::PkgEntry};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Selects extraction-relative paths. A directory selects its descendants.
/// Empty `paths` selects everything in the requested scope; globs are not used.
#[derive(Debug, Default, Clone)]
pub struct ExtractOptions {
    pub metadata_only: bool,
    pub paths: Vec<String>,
}

/// A path that could not be extracted or checked.
#[derive(Debug, Clone)]
pub struct SkippedEntry {
    pub path: String,
    pub reason: String,
}

/// Completed files and omissions. Callers must check `skipped` before reporting
/// full success. Counts include both metadata entries and filesystem files.
#[derive(Debug, Default)]
pub struct ExtractionReport {
    pub files: usize,
    pub bytes: u64,
    pub skipped: Vec<SkippedEntry>,
}

/// A metadata entry or filesystem item. Paths use `/` on every platform and
/// match full extraction destinations, including unknown metadata IDs.
#[derive(Debug, Clone)]
pub struct ListedItem {
    pub path: Option<String>,
    pub is_directory: bool,
    pub size: u64,
    pub entry_id: Option<u32>,
    pub encrypted: bool,
    pub key_index: Option<usize>,
}

/// Extracts and inspects a PKG using an optional progress reporter.
pub struct PkgExtractor<'a, R: AsRef<[u8]> + Sync, P: ExtractProgress> {
    pkg: &'a Pkg<R>,
    progress: P,
    overwrite: bool,
}

struct EntryWork {
    index: usize,
    entry: PkgEntry,
    path: Option<PathBuf>,
}

struct Plan<'a> {
    entries: Vec<EntryWork>,
    filesystem: Filesystem<'a>,
}

impl<'a, R: AsRef<[u8]> + Sync, P: ExtractProgress> PkgExtractor<'a, R, P> {
    /// If `overwrite` is true, replace each file only after successfully writing it.
    pub fn new(pkg: &'a Pkg<R>, progress: P, overwrite: bool) -> Self {
        Self {
            pkg,
            progress,
            overwrite,
        }
    }

    /// Extracts metadata to `sce_sys/` and filesystem contents to the output root.
    /// Returns an error when any entries are skipped; completed files remain.
    pub fn extract(&self, output: impl AsRef<Path>) -> Result<(), ExtractError> {
        complete(self.extract_with_options(output, &ExtractOptions::default())?)
    }

    /// Extracts only metadata, directly into the supplied directory.
    pub fn extract_entries(&self, output: impl AsRef<Path>) -> Result<(), ExtractError> {
        let plan = self.plan(
            &ExtractOptions {
                metadata_only: true,
                paths: vec![],
            },
            false,
        )?;
        complete(self.run(plan, Some(output.as_ref()))?)
    }

    /// Extracts only filesystem contents.
    pub fn extract_pfs(&self, output: impl AsRef<Path>) -> Result<(), ExtractError> {
        let plan = Plan {
            entries: vec![],
            filesystem: read_filesystem(self.pkg)?,
        };
        complete(self.run(plan, Some(output.as_ref()))?)
    }

    /// Extracts selected paths, returning an explicit report of any omissions.
    pub fn extract_with_options(
        &self,
        output: impl AsRef<Path>,
        options: &ExtractOptions,
    ) -> Result<ExtractionReport, ExtractError> {
        let plan = self.plan(options, true)?;
        self.run(plan, Some(output.as_ref()))
    }

    /// Reads and decompresses selected data without writing output files.
    /// This does not validate signatures, MACs, licenses, or authenticity.
    pub fn verify(&self, options: &ExtractOptions) -> Result<ExtractionReport, ExtractError> {
        let plan = self.plan(options, true)?;
        self.run(plan, None)
    }

    /// Checks stored PKG digests and (unless metadata-only) outer/inner PFS HMACs.
    /// Missing signatures are counted separately and never reported as verified.
    pub fn verify_integrity(&self, metadata_only: bool) -> Result<IntegrityReport, ExtractError> {
        self.check_cancelled()?;
        self.progress.phase("Checking stored PKG digests");
        let digests = self
            .pkg
            .verify_integrity(!metadata_only, || self.progress.is_cancelled())
            .map_err(integrity_error)?;
        let mut result = IntegrityReport {
            digests: digests.checked,
            macs: 0,
            missing: digests.missing,
        };
        if !metadata_only {
            self.progress.phase("Checking PFS block HMACs");
            let (outer, inner) = filesystem::open_filesystems(self.pkg)?;
            let image = self.pkg.get_pfs_image().ok_or(ExtractError::NoPfsImage)?;
            let key = (!image.ekpfs.is_empty()).then_some(image.ekpfs);
            for pfs in [outer, inner] {
                let report = pfs
                    .verify_integrity(key, || self.progress.is_cancelled())
                    .map_err(integrity_error)?;
                result.macs += report.checked;
                result.missing += report.missing;
            }
        }
        Ok(result)
    }

    /// Lists the PKG entry table without loading encryption keys.
    pub fn list_entries(&self) -> Result<Vec<ListedItem>, ExtractError> {
        self.entries(true)?
            .into_iter()
            .map(|work| {
                Ok(ListedItem {
                    path: work.path.as_deref().map(portable_path),
                    is_directory: false,
                    size: work.entry.data_size() as u64,
                    entry_id: Some(work.entry.id()),
                    encrypted: work.entry.is_encrypted(),
                    key_index: work.entry.is_encrypted().then(|| work.entry.key_index()),
                })
            })
            .collect()
    }

    /// Lists filesystem paths without extracting their contents.
    pub fn list_files(&self) -> Result<Vec<ListedItem>, ExtractError> {
        let filesystem = read_filesystem(self.pkg)?;
        let mut items: Vec<_> = filesystem
            .dirs
            .iter()
            .map(|path| ListedItem {
                path: Some(portable_path(path)),
                is_directory: true,
                size: 0,
                entry_id: None,
                encrypted: false,
                key_index: None,
            })
            .collect();
        items.extend(filesystem.files.iter().map(|work| ListedItem {
            path: Some(portable_path(&work.output_path)),
            is_directory: false,
            size: work.file.len(),
            entry_id: None,
            encrypted: false,
            key_index: None,
        }));
        items.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(items)
    }

    fn entries(&self, prefix: bool) -> Result<Vec<EntryWork>, ExtractError> {
        self.pkg
            .entries()
            .enumerate()
            .map(|(index, result)| {
                let (_, entry) = result
                    .map_err(|source| ExtractError::ReadEntryFailed { num: index, source })?;
                Ok(EntryWork {
                    index,
                    path: Some(
                        Path::new(if prefix { "sce_sys" } else { "" }).join(
                            self.pkg
                                .entry_path(&entry)
                                .map_err(|source| ExtractError::EntryName { source })?,
                        ),
                    ),
                    entry,
                })
            })
            .collect()
    }

    fn plan(&self, options: &ExtractOptions, prefix: bool) -> Result<Plan<'a>, ExtractError> {
        self.check_cancelled()?;
        let selection = Selection::new(&options.paths)?;
        self.progress.phase("Reading package entries");
        let mut entries = self.entries(prefix)?;
        if !options.metadata_only {
            self.progress.phase("Reading filesystem");
        }
        let mut filesystem = if options.metadata_only {
            Filesystem {
                dirs: vec![],
                files: vec![],
            }
        } else {
            read_filesystem(self.pkg)?
        };
        let known: Vec<_> = entries
            .iter()
            .filter_map(|e| e.path.as_ref())
            .chain(filesystem.dirs.iter())
            .chain(filesystem.files.iter().map(|f| &f.output_path))
            .collect();
        for selected in &selection.paths {
            if !known.iter().any(|path| path.starts_with(selected)) {
                return Err(ExtractError::PathNotFound {
                    path: portable_path(selected),
                });
            }
        }
        entries.retain(|entry| {
            entry
                .path
                .as_ref()
                .map(|p| selection.includes(p))
                .unwrap_or(selection.paths.is_empty())
        });
        filesystem
            .files
            .retain(|file| selection.includes(&file.output_path));
        filesystem.dirs.retain(|dir| {
            selection.includes(dir)
                || filesystem
                    .files
                    .iter()
                    .any(|file| file.output_path.starts_with(dir))
        });
        let mut destinations = HashSet::new();
        for path in entries
            .iter()
            .filter_map(|entry| entry.path.as_ref())
            .chain(filesystem.files.iter().map(|file| &file.output_path))
        {
            if !destinations.insert(portable_path(path).to_lowercase()) {
                return Err(ExtractError::DuplicatePath {
                    path: portable_path(path),
                });
            }
        }
        Ok(Plan {
            entries,
            filesystem,
        })
    }

    fn check_cancelled(&self) -> Result<(), ExtractError> {
        if self.progress.is_cancelled() {
            Err(ExtractError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn run(
        &self,
        plan: Plan<'_>,
        output_path: Option<&Path>,
    ) -> Result<ExtractionReport, ExtractError> {
        self.check_cancelled()?;
        let output = output_path.map(Output::new).transpose()?;
        // Detect known conflicts before writing any files; publication also checks
        // atomically to catch destinations created after this preflight.
        if let Some(output) = &output {
            for path in plan
                .entries
                .iter()
                .filter_map(|entry| entry.path.as_ref())
                .chain(plan.filesystem.files.iter().map(|work| &work.output_path))
            {
                output.check_file(path, self.overwrite)?;
            }
            for dir in &plan.filesystem.dirs {
                output.create_dir(dir)?;
            }
        }
        let mut report = ExtractionReport::default();
        let total_bytes = plan
            .entries
            .iter()
            .map(|entry| entry.entry.data_size() as u64)
            .chain(plan.filesystem.files.iter().map(|work| work.file.len()))
            .try_fold(0u64, u64::checked_add)
            .ok_or(ExtractError::SizeOverflow)?;
        self.progress.bytes_total(total_bytes);
        self.progress.phase(if output.is_some() {
            "Extracting metadata"
        } else {
            "Checking metadata"
        });
        let mut extracted_entries = 0;
        for work in &plan.entries {
            self.check_cancelled()?;
            let path_label = work
                .path
                .as_deref()
                .map(portable_path)
                .unwrap_or_else(|| format!("entry 0x{:08X}", work.entry.id()));
            if work.path.is_none() && output.is_some() {
                self.skip(
                    &mut report,
                    path_label,
                    "unknown entry ID; no output path".into(),
                );
                continue;
            }
            let data = match self.pkg.entry_data(&work.entry) {
                Ok(data) => data,
                Err(orbis_pkg::EntryDataError::NoDecryptionKey { key_index }) => {
                    self.skip(
                        &mut report,
                        path_label,
                        format!("no decryption key for slot {key_index}"),
                    );
                    continue;
                }
                Err(source) => {
                    return Err(ExtractError::GetEntryDataFailed {
                        num: work.index,
                        source,
                    });
                }
            };
            if let Some(output) = &output {
                let path = work.path.as_ref().unwrap();
                self.progress
                    .entry_start(path, work.index, plan.entries.len());
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    output.create_dir(parent)?;
                }
                let mut file = output.create_file(path, self.overwrite)?;
                for chunk in data.chunks(1024 * 1024) {
                    self.check_cancelled()?;
                    file.write_all(chunk)
                        .map_err(|source| ExtractError::WriteFailed {
                            path: output.path.join(path),
                            source,
                        })?;
                    self.progress.bytes_processed(chunk.len() as u64);
                }
                self.check_cancelled()?;
                file.commit()?;
            } else {
                self.progress.bytes_processed(data.len() as u64);
            }
            extracted_entries += 1;
            report.files += 1;
            report.bytes += data.len() as u64;
        }
        self.progress
            .entries_completed(extracted_entries, report.skipped.len());
        self.progress.phase(if output.is_some() {
            "Extracting filesystem"
        } else {
            "Checking filesystem"
        });
        self.progress.pfs_start(plan.filesystem.files.len());
        let results: Result<Vec<u64>, ExtractError> = plan
            .filesystem
            .files
            .par_iter()
            .map(|work| {
                self.check_cancelled()?;
                self.progress.pfs_file(&work.output_path, work.file.len());
                process_file(work, output.as_ref(), &self.progress, self.overwrite)
            })
            .collect();
        for bytes in results? {
            report.files += 1;
            report.bytes += bytes;
        }
        self.progress.pfs_completed();
        Ok(report)
    }

    fn skip(&self, report: &mut ExtractionReport, path: String, reason: String) {
        self.progress.entry_skipped(Path::new(&path), &reason);
        report.skipped.push(SkippedEntry { path, reason });
    }
}

fn complete(report: ExtractionReport) -> Result<(), ExtractError> {
    if report.skipped.is_empty() {
        Ok(())
    } else {
        Err(ExtractError::Incomplete {
            skipped: report.skipped.len(),
        })
    }
}

fn portable_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

struct Selection {
    paths: Vec<PathBuf>,
}
impl Selection {
    fn new(paths: &[String]) -> Result<Self, ExtractError> {
        let mut normalized = Vec::new();
        for path in paths {
            let path = path.trim_end_matches('/');
            if path.is_empty() || !path.split('/').all(valid_name) {
                return Err(ExtractError::InvalidSelection {
                    path: path.to_owned(),
                });
            }
            normalized.push(path.split('/').collect());
        }
        Ok(Self { paths: normalized })
    }

    fn includes(&self, path: &Path) -> bool {
        self.paths.is_empty() || self.paths.iter().any(|selected| path.starts_with(selected))
    }
}

fn process_file<P: ExtractProgress>(
    work: &FileWork<'_>,
    output: Option<&Output>,
    progress: &P,
    overwrite: bool,
) -> Result<u64, ExtractError> {
    let source: Box<dyn Image + '_> = if work.file.is_compressed() {
        Box::new(
            orbis_pfs::pfsc::PfscImage::open(work.file.clone().into_image()).map_err(|source| {
                ExtractError::DecompressFileFailed {
                    path: work.pfs_path.clone(),
                    source,
                }
            })?,
        )
    } else {
        Box::new(work.file.clone().into_image())
    };
    if source.len() != work.file.len() {
        return Err(ExtractError::FileSizeMismatch {
            path: work.pfs_path.clone(),
            expected: work.file.len(),
            actual: source.len(),
        });
    }
    let mut dest = output
        .map(|out| out.create_file(&work.output_path, overwrite))
        .transpose()?;
    let mut buffer = vec![0u8; 8 * 1024 * 1024];
    let mut offset = 0;
    loop {
        if progress.is_cancelled() {
            return Err(ExtractError::Cancelled);
        }
        let read = match source.read_at(offset, &mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(source) => {
                return Err(ExtractError::ReadPfsFileFailed {
                    path: work.pfs_path.clone(),
                    source,
                });
            }
        };
        if let Some(dest) = &mut dest {
            dest.write_all(&buffer[..read])
                .map_err(|source| ExtractError::WriteFailed {
                    path: output.unwrap().path.join(&work.output_path),
                    source,
                })?;
        }
        offset += read as u64;
        progress.bytes_processed(read as u64);
    }
    if offset != source.len() {
        return Err(ExtractError::FileSizeMismatch {
            path: work.pfs_path.clone(),
            expected: source.len(),
            actual: offset,
        });
    }
    if progress.is_cancelled() {
        return Err(ExtractError::Cancelled);
    }
    if let Some(dest) = dest {
        dest.commit()?;
    }
    progress.pfs_file_completed(offset);
    Ok(offset)
}

/// Stored integrity checks, separate from readability/decompression checks.
#[derive(Debug, Default)]
pub struct IntegrityReport {
    pub digests: usize,
    pub macs: usize,
    pub missing: usize,
}

fn integrity_error(source: io::Error) -> ExtractError {
    if source.kind() == io::ErrorKind::Interrupted {
        ExtractError::Cancelled
    } else {
        ExtractError::Integrity { source }
    }
}
