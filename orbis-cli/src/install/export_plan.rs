//! Common source preparation for previews, staged exports, and resumable exports.
use super::*;
use orbis_pkg::sfo::AppMetadata;

struct UpdateSource {
    dir: Dir,
    version: String,
    files: BTreeMap<String, FileRecord>,
}

pub(super) struct PreparedExport {
    metadata: AppMetadata,
    base: Dir,
    update: Option<UpdateSource>,
    pub entries: BTreeMap<String, Entry>,
    pub report: ExportReport,
}

impl PreparedExport {
    pub fn new(
        location: &storage::Layout,
        destination: PathBuf,
        progress: &impl ExtractProgress,
    ) -> Result<Self> {
        location.require_recovered()?;
        let metadata = location.base_metadata()?;
        let manifest = location.active_manifest(&location.state()?, &metadata, progress)?;
        let base = location.base_dir()?;
        let update = manifest
            .map(|manifest| -> Result<_> {
                Ok(UpdateSource {
                    dir: location.parent.open_dir(&location.active)?,
                    version: manifest.app_version,
                    files: manifest
                        .files
                        .into_iter()
                        .map(|f| (f.path.clone(), f))
                        .collect(),
                })
            })
            .transpose()?;
        progress.phase("Planning merged export");
        let mut entries = BTreeMap::new();
        scan(&base, Layer::Base, &mut entries, progress)?;
        if let Some(update) = &update {
            scan(&update.dir, Layer::Update, &mut entries, progress)?;
        }
        let mut report = ExportReport {
            title_id: metadata.title_id.clone(),
            app_version: update
                .as_ref()
                .map_or_else(|| metadata.version.to_string(), |u| u.version.clone()),
            destination,
            files: 0,
            bytes: 0,
            update_files: 0,
        };
        for entry in entries.values().filter(|e| !e.directory) {
            report.files += 1;
            report.update_files += usize::from(entry.layer == Layer::Update);
            report.bytes = report
                .bytes
                .checked_add(entry.bytes)
                .ok_or_else(|| invalid("export size overflows"))?;
        }
        if update.as_ref().map_or(0, |u| u.files.len()) != report.update_files {
            return Err(invalid("update changed during export"));
        }
        Ok(Self {
            metadata,
            base,
            update,
            entries,
            report,
        })
    }

    pub fn files(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values().filter(|e| !e.directory)
    }

    pub fn filesystem_plan(&self, parent: &Path) -> Result<super::super::FilesystemPlan> {
        super::super::FilesystemPlan::inspect(
            parent,
            self.files().map(|e| e.bytes),
            self.entries.len(),
        )
    }

    pub fn preview(&self, filesystem: super::super::FilesystemPlan) -> ExportPlan {
        ExportPlan {
            schema_version: 1,
            title_id: self.report.title_id.clone(),
            base_version: self.metadata.version.to_string(),
            update_version: self.update.as_ref().map(|u| u.version.clone()),
            destination: self.report.destination.clone(),
            files: self.report.files,
            update_files: self.report.update_files,
            bytes: self.report.bytes,
            filesystem,
        }
    }

    pub fn source(&self, entry: &Entry) -> Result<(&Dir, Option<&FileRecord>)> {
        match entry.layer {
            Layer::Base => Ok((&self.base, None)),
            Layer::Update => {
                let update = self
                    .update
                    .as_ref()
                    .ok_or_else(|| invalid("missing update source"))?;
                let record = update
                    .files
                    .get(&entry.path)
                    .ok_or_else(|| invalid("update changed during export"))?;
                Ok((&update.dir, Some(record)))
            }
        }
    }

    pub fn source_manifest(&self, progress: &impl ExtractProgress) -> Result<ExportManifest> {
        progress.phase("Hashing export sources");
        progress.bytes_total(self.report.bytes);
        let mut files = Vec::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        for entry in self.files() {
            let (source, expected) = self.source(entry)?;
            let record = crate::inventory::hash_file(source, &entry.path, &mut buffer, progress)?;
            if record.bytes != entry.bytes || expected.is_some_and(|expected| expected != &record) {
                return Err(invalid(format!(
                    "source changed while planning export: {}",
                    entry.path
                )));
            }
            files.push(record);
        }
        Ok(self.manifest(files, false))
    }

    pub fn manifest(&self, files: Vec<FileRecord>, complete: bool) -> ExportManifest {
        ExportManifest {
            schema_version: 1,
            title_id: self.metadata.title_id.clone(),
            content_id: self.metadata.content_id.clone(),
            base_version: self.metadata.version.to_string(),
            update_version: self.update.as_ref().map(|u| u.version.clone()),
            complete,
            directories: self
                .entries
                .values()
                .filter(|e| e.directory)
                .map(|e| e.path.clone())
                .collect(),
            files,
        }
    }
}
