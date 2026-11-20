//! Common source preparation for previews, staged exports, and resumable exports.
use super::*;
use orbis_pkg::sfo::AppMetadata;

struct UpdateSource {
    dir: Dir,
    manifest: storage::Manifest,
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
        let (base, metadata) = location.original_base()?;
        let update = location
            .open_active_manifest(&location.state()?)?
            .map(|(dir, manifest)| UpdateSource { dir, manifest });
        progress.phase("Planning merged export");
        let mut entries = BTreeMap::new();
        scan(&base, Layer::Base, &mut entries, progress)?;
        if let Some(update) = &update {
            storage::validate_manifest_metadata(&update.dir, &update.manifest, &metadata)?;
            scan(&update.dir, Layer::Update, &mut entries, progress)?;
        }
        let mut report = ExportReport {
            title_id: metadata.title_id.clone(),
            app_version: update.as_ref().map_or_else(
                || metadata.version.to_string(),
                |u| u.manifest.app_version.clone(),
            ),
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
        Ok(Self {
            metadata,
            base,
            update,
            entries,
            report,
        })
    }

    pub fn verify_update(&self, progress: &impl ExtractProgress) -> Result<()> {
        if let Some(update) = &self.update {
            progress.phase("Checking installed update");
            storage::verify_manifest(&update.dir, &update.manifest, &self.metadata, progress)?;
            if update.manifest.files.len() != self.report.update_files {
                return Err(invalid("update changed during export"));
            }
        }
        Ok(())
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

    pub(super) fn preview_new_destination(
        &self,
        filesystem: super::super::FilesystemPlan,
        resume: bool,
        for_confirmation: bool,
        progress: &impl ExtractProgress,
    ) -> Result<(ExportEstimate, Option<ExportManifest>)> {
        let work = resume.then_some(ResumePlan {
            reusable_files: 0,
            remaining_files: self.report.files,
            remaining_bytes: self.report.bytes,
        });
        let estimate = ExportEstimate {
            plan: self.preview(filesystem),
            resume: work,
        };
        if estimate.plan.filesystem.shortfall_bytes() > 0 {
            return Ok((estimate, None));
        }
        self.verify_update(progress)?;
        let manifest = for_confirmation
            .then(|| self.source_manifest(progress))
            .transpose()?;
        Ok((estimate, manifest))
    }

    pub fn preview(&self, filesystem: super::super::FilesystemPlan) -> ExportPlan {
        ExportPlan {
            schema_version: 1,
            title_id: self.report.title_id.clone(),
            base_version: self.metadata.version.to_string(),
            update_version: self.update.as_ref().map(|u| u.manifest.app_version.clone()),
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
                // Inventory verification checks that the manifest is sorted by path.
                let files = &update.manifest.files;
                let index = files
                    .binary_search_by(|f| f.path.cmp(&entry.path))
                    .map_err(|_| invalid("update changed during export"))?;
                Ok((&update.dir, Some(&files[index])))
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
            update_version: self.update.as_ref().map(|u| u.manifest.app_version.clone()),
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
