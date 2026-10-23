//! One full-file update merged into an original base. The journal stores file
//! inventories before moving any originals. Recovery always restores that base;
//! it never guesses how omissions in a later patch should be interpreted.
use super::*;
use crate::inventory::{FileRecord, hash_file};
use std::collections::BTreeMap;
use storage::{exists, read_json, sync_dir, write_json};

mod files;
use files::{checked_dir, exclusive_move, move_file, namespace, parents};

const ACTIVE: &str = "merge";
const PREPARING: &str = "merge-preparing";
const RETIRED: &str = "merge-retired";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MergeManifest {
    schema_version: u32,
    title_id: String,
    content_id: String,
    base_version: String,
    update_version: String,
    files: Vec<Replacement>,
    created_directories: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Replacement {
    before: Option<FileRecord>,
    after: FileRecord,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Pending {
    Apply,
    Restore,
}

#[derive(Debug, Serialize)]
pub struct MergeStatus {
    pub title_id: String,
    pub base_version: String,
    pub update_version: String,
    pub pending_recovery: bool,
}

pub(super) fn present(layout: &Layout) -> Result<bool> {
    layout
        .root
        .as_ref()
        .map_or(Ok(false), |r| exists(r, ACTIVE))
}
pub(super) fn pending(layout: &Layout) -> Result<bool> {
    if !present(layout)? {
        return Ok(false);
    }
    let dir = checked_dir(layout.manager()?, Path::new(ACTIVE))?;
    exists(&dir, "pending.json")
}

// Read-only status shares the writer's lock, including while PARAM.SFO is between
// its backup and publish renames. Opening an untouched game creates no files.
struct ReadLock(std::fs::File);
impl Drop for ReadLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
fn read_lock(layout: &Layout) -> Result<Option<ReadLock>> {
    let Some(root) = &layout.root else {
        return Ok(None);
    };
    if !exists(root, "lock")? {
        return Ok(None);
    }
    if !root.symlink_metadata("lock")?.is_file() {
        return Err(invalid("invalid installation lock"));
    }
    let file = root.open("lock")?.into_std();
    fs2::FileExt::try_lock_shared(&file)
        .map_err(|_| invalid("another installation operation holds the lock"))?;
    Ok(Some(ReadLock(file)))
}

/// Reports a managed merge without requiring the original PKG or reading the
/// currently installed PARAM.SFO, which may be absent during crash recovery.
pub fn merge_status(base: &Path) -> Result<Option<MergeStatus>> {
    let layout = Layout::inspect(base)?;
    let _guard = read_lock(&layout)?;
    if !present(&layout)? {
        return Ok(None);
    }
    let (_, manifest) = load(&layout)?;
    Ok(Some(MergeStatus {
        title_id: manifest.title_id,
        base_version: manifest.base_version,
        update_version: manifest.update_version,
        pending_recovery: pending(&layout)?,
    }))
}

pub(super) fn status(
    base: &Path,
    progress: &impl ExtractProgress,
) -> Result<Option<InstallationStatus>> {
    let layout = Layout::inspect(base)?;
    // Leave non-merged games to the separate-update status reader, including
    // its structured diagnostics for a busy or invalid installation lock.
    if !present(&layout)? {
        return Ok(None);
    }
    let _guard = read_lock(&layout)?;
    // Restoration may have finished between the initial probe and locking.
    if !present(&layout)? {
        return Ok(None);
    }
    let (dir, manifest) = load(&layout)?;
    let pending_recovery = pending(&layout)?;
    let mut issues = Vec::new();
    if pending_recovery {
        issues.push("interrupted merge; run recover --base <DIR> to restore the base".into());
    } else if let Err(e) = verify(&layout, &dir, &manifest, true, progress) {
        if progress.is_cancelled() {
            return Err(e);
        }
        issues.push(e.to_string());
    }
    let unmanaged_directories = if exists(&layout.parent, &layout.active)? {
        vec![layout.active_path()]
    } else {
        Vec::new()
    };
    Ok(Some(InstallationStatus {
        schema_version: 1,
        mode: InstallMode::Merge,
        title_id: manifest.title_id,
        base_version: manifest.base_version.clone(),
        active_version: Some(manifest.update_version),
        rollback: "base".into(),
        rollback_version: Some(manifest.base_version),
        pending_recovery,
        unmanaged_directories,
        modified_files: Vec::new(),
        issues,
        base_integrity: "merge_paths_only".into(),
    }))
}

pub(super) fn plan<R: AsRef<[u8]> + Sync>(
    pkg: &Pkg<R>,
    base: &Path,
    progress: impl ExtractProgress,
) -> Result<InstallPlan> {
    let layout = Layout::open(base, false)?;
    let _guard = read_lock(&layout)?;
    make_plan(pkg, &layout, &progress)
}
fn make_plan<R: AsRef<[u8]> + Sync>(
    pkg: &Pkg<R>,
    layout: &Layout,
    progress: &impl ExtractProgress,
) -> Result<InstallPlan> {
    layout.require_recovered()?;
    if present(layout)? {
        return Err(invalid(
            "restore the merged base before applying another update",
        ));
    }
    let mut legacy = layout.base.clone();
    legacy.push("-PATCH");
    if exists(&layout.parent, &legacy)?
        || exists(&layout.parent, &layout.active)?
        || layout.state()?.active
    {
        return Err(invalid(
            "merge requires an original base without a separate update; roll back or move the sibling -UPDATE folder first",
        ));
    }
    let mut plan = super::make_plan(pkg, layout, progress)?;
    progress.phase("Checking game paths");
    let base = layout.base_dir()?;
    let names = namespace(&base, progress)?;
    let mut combined = BTreeMap::new();
    for (path, directory) in &names {
        files::insert_name(&mut combined, path, *directory)?;
    }
    let extractor = PkgExtractor::new(pkg, SilentProgress, false);
    let mut entries = extractor.list_entries()?;
    entries.extend(extractor.list_files()?);
    for entry in entries {
        cancelled(progress)?;
        files::insert_name(&mut combined, &entry.path.unwrap(), entry.is_directory)?;
    }
    let mut backup_bytes = 0u64;
    for file in &plan.files {
        if names.get(&file.path) == Some(&false) {
            backup_bytes = backup_bytes
                .checked_add(base.symlink_metadata(&file.path)?.len())
                .ok_or_else(|| invalid("backup size overflows"))?;
        }
    }
    plan.mode = InstallMode::Merge;
    plan.backup_bytes = backup_bytes;
    plan.destination = layout.parent_path.join(&layout.base);
    Ok(plan)
}

pub(super) fn install<R: AsRef<[u8]> + Sync>(
    pkg: &Pkg<R>,
    base: &Path,
    progress: impl ExtractProgress,
) -> Result<InstallPlan> {
    let layout = Layout::open(base, true)?;
    let plan = make_plan(pkg, &layout, &progress)?;
    check_space(plan.required_free_bytes, plan.available_bytes)?;
    cleanup(&layout)?;
    let root = layout.manager()?;
    root.create_dir(PREPARING)?;
    let result = (|| {
        let stage = root.open_dir(PREPARING)?;
        files::probe(&stage)?;
        stage.create_dir("incoming")?;
        stage.create_dir("originals")?;
        let report = PkgExtractor::new(pkg, &progress, false).extract_with_options(
            layout
                .parent_path
                .join(&layout.root_name)
                .join(PREPARING)
                .join("incoming"),
            &ExtractOptions::default(),
        )?;
        if !report.skipped.is_empty() {
            return Err(invalid(
                "update extraction was incomplete; the base was preserved",
            ));
        }
        let incoming = stage.open_dir("incoming")?;
        let originals = stage.open_dir("originals")?;
        let base = layout.base_dir()?;
        let metadata = storage::app_metadata(&incoming)?;
        if metadata.content_id != plan.content_id
            || metadata.title_id != plan.title_id
            || metadata.version.to_string() != plan.update_version
            || metadata.category != "gp"
        {
            return Err(invalid("staged PARAM.SFO differs from the merge plan"));
        }
        progress.phase("Checking update and original files");
        progress.bytes_total(plan.extracted_bytes.saturating_add(plan.backup_bytes));
        let records = inventory(&incoming, &progress)?;
        if records
            .iter()
            .map(|f| &f.path)
            .ne(plan.files.iter().map(|f| &f.path))
        {
            return Err(invalid("staged file paths differ from the merge plan"));
        }
        let staged_names = namespace(&incoming, &progress)?;
        let base_names = namespace(&base, &progress)?;
        let mut combined = BTreeMap::new();
        for (path, directory) in base_names.iter().chain(&staged_names) {
            files::insert_name(&mut combined, path, *directory)?;
        }
        let created_directories = staged_names
            .iter()
            .filter(|(p, d)| **d && !base_names.contains_key(*p))
            .map(|(p, _)| p.clone())
            .collect();
        let mut files = Vec::new();
        let mut buffer = vec![0; 1024 * 1024];
        for after in records {
            let before = if exists(&base, &after.path)? {
                Some(hash_file(&base, &after.path, &mut buffer, &progress)?)
            } else {
                None
            };
            parents(&originals, &after.path)?;
            files.push(Replacement { before, after });
        }
        let manifest = MergeManifest {
            schema_version: 1,
            title_id: plan.title_id.clone(),
            content_id: plan.content_id.clone(),
            base_version: plan.current_version.clone(),
            update_version: plan.update_version.clone(),
            files,
            created_directories,
        };
        write_json(&stage, "manifest.json", &manifest)?;
        write_json(&stage, "pending.json", &Pending::Apply)?;
        // Extracted files are synced by the extractor; persist every directory
        // entry before the journal can authorize moving originals out of base.
        for (path, directory) in staged_names.iter().rev() {
            if *directory {
                sync_dir(&checked_dir(&incoming, Path::new(path))?)?;
            }
        }
        sync_dir(&incoming)?;
        sync_dir(&originals)?;
        sync_dir(&stage)?;
        cancelled(&progress)?;
        drop(incoming);
        drop(originals);
        drop(stage);
        exclusive_move(root, PREPARING.as_ref(), root, ACTIVE.as_ref())?;
        sync_dir(root)?;
        checkpoint("journal");
        let (dir, manifest) = load(&layout)?;
        apply(&layout, &dir, &manifest, &progress)?;
        cancelled(&progress)?;
        dir.remove_file("pending.json")?; // Commit: recovery now keeps this merge.
        if let Err(e) = sync_dir(&dir) {
            progress.warning(&format!("merge committed, but final sync failed: {e}"));
        }
        checkpoint("committed");
        Ok(())
    })();
    if let Err(error) = result {
        if let Err(recovery) = recover(&layout, &NonCancelling(&progress)) {
            return Err(invalid(format!(
                "{error}; automatic restoration could not finish: {recovery}. Keep the game closed and run recover --base <DIR>"
            )));
        }
        return Err(error);
    }
    Ok(plan)
}

fn apply(
    layout: &Layout,
    dir: &Dir,
    manifest: &MergeManifest,
    progress: &impl ExtractProgress,
) -> Result<()> {
    let base = layout.base_dir()?;
    let incoming = checked_dir(dir, Path::new("incoming"))?;
    let originals = checked_dir(dir, Path::new("originals"))?;
    progress.phase("Merging update into game folder");
    for path in &manifest.created_directories {
        cancelled(progress)?;
        let parent = checked_dir(&base, Path::new(path).parent().unwrap())?;
        parent.create_dir(Path::new(path).file_name().unwrap())?;
        sync_dir(&parent)?;
    }
    let mut buffer = vec![0; 1024 * 1024];
    for file in &manifest.files {
        cancelled(progress)?;
        let path = &file.after.path;
        if let Some(before) = &file.before {
            require_record(&base, before, &mut buffer, progress)?;
            move_file(&base, &originals, path)?;
            checkpoint("backup");
        } else if exists(&base, path)? {
            return Err(invalid(format!("game file appeared during merge: {path}")));
        }
        move_file(&incoming, &base, path)?;
        checkpoint("publish");
    }
    Ok(())
}

fn load(layout: &Layout) -> Result<(Dir, MergeManifest)> {
    let dir = checked_dir(layout.manager()?, Path::new(ACTIVE))?;
    let manifest: MergeManifest = read_json(&dir, "manifest.json")?;
    if manifest.schema_version != 1
        || manifest.files.is_empty()
        || manifest.base_version.parse::<AppVersion>()?
            >= manifest.update_version.parse::<AppVersion>()?
    {
        return Err(invalid("invalid merge manifest version"));
    }
    let mut names = BTreeMap::new();
    let mut previous = None;
    for file in &manifest.files {
        let path = &file.after.path;
        if previous.is_some_and(|p: &String| p >= path) {
            return Err(invalid("unordered or duplicate merge paths"));
        }
        previous = Some(path);
        files::insert_name(&mut names, path, false)?;
        for record in std::iter::once(&file.after).chain(file.before.iter()) {
            if record.path != *path
                || record.sha256.len() != 64
                || !record.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(invalid("invalid merge file record"));
            }
        }
    }
    let sfo = manifest
        .files
        .iter()
        .find(|f| f.after.path == "sce_sys/param.sfo");
    if !sfo.is_some_and(|f| f.before.is_some()) {
        return Err(invalid("merge manifest lacks original PARAM.SFO"));
    }
    let mut previous = None;
    for path in &manifest.created_directories {
        if previous.is_some_and(|p: &String| p >= path) {
            return Err(invalid("unordered or duplicate merge directories"));
        }
        previous = Some(path);
        files::insert_name(&mut names, path, true)?;
        if manifest
            .files
            .iter()
            .any(|f| f.before.is_some() && f.after.path.starts_with(&format!("{path}/")))
        {
            return Err(invalid("new merge directory contains an original file"));
        }
    }
    if exists(&dir, "pending.json")? {
        let _: Pending = read_json(&dir, "pending.json")?;
    }
    Ok((dir, manifest))
}

fn require_record(
    root: &Dir,
    record: &FileRecord,
    buffer: &mut [u8],
    progress: &impl ExtractProgress,
) -> Result<()> {
    if hash_file(root, &record.path, buffer, progress)? != *record {
        return Err(invalid(format!(
            "file changed: {}; preserve your changes before restoring the base",
            record.path
        )));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum FileState {
    Original,
    BackedUp,
    Published,
}

/// Validate the complete transaction before changing anything during restoration.
/// Incoming/original presence distinguishes every point between the two renames.
fn verify(
    layout: &Layout,
    dir: &Dir,
    manifest: &MergeManifest,
    committed: bool,
    progress: &impl ExtractProgress,
) -> Result<Vec<FileState>> {
    let base = layout.base_dir()?;
    let incoming = checked_dir(dir, Path::new("incoming"))?;
    let originals = checked_dir(dir, Path::new("originals"))?;
    let mut buffer = vec![0; 1024 * 1024];
    let mut states = Vec::new();
    progress.phase("Checking merged files and backups");
    for file in &manifest.files {
        cancelled(progress)?;
        let path = &file.after.path;
        let has_incoming = exists(&incoming, path)?;
        let has_original = exists(&originals, path)?;
        let has_base = exists(&base, path)?;
        let state = match (&file.before, has_original, has_incoming, has_base) {
            (Some(before), false, true, true) if !committed => {
                require_record(&base, before, &mut buffer, progress)?;
                FileState::Original
            }
            (None, false, true, false) if !committed => FileState::Original,
            (Some(before), true, true, false) if !committed => {
                require_record(&originals, before, &mut buffer, progress)?;
                FileState::BackedUp
            }
            (before, backup, false, true) if before.is_some() == backup => {
                require_record(&base, &file.after, &mut buffer, progress)?;
                if let Some(before) = before {
                    require_record(&originals, before, &mut buffer, progress)?;
                }
                FileState::Published
            }
            _ => {
                return Err(invalid(format!(
                    "merge files are missing or conflicting: {path}; keep backups and resolve the conflict before recovery"
                )));
            }
        };
        if has_incoming {
            require_record(&incoming, &file.after, &mut buffer, progress)?;
        }
        states.push(state);
    }
    // Check identity against both saved inventories, even while PARAM.SFO is
    // temporarily absent from the game directory after a crash.
    let index = manifest
        .files
        .iter()
        .position(|f| f.after.path == "sce_sys/param.sfo")
        .unwrap();
    let original = if matches!(states[index], FileState::Original) {
        &base
    } else {
        &originals
    };
    let updated = if matches!(states[index], FileState::Published) {
        &base
    } else {
        &incoming
    };
    for (root, version, category) in [
        (original, &manifest.base_version, "gd"),
        (updated, &manifest.update_version, "gp"),
    ] {
        let meta = storage::app_metadata(root)?;
        if meta.title_id != manifest.title_id
            || meta.content_id != manifest.content_id
            || meta.version.to_string() != *version
            || meta.category != category
        {
            return Err(invalid("merge metadata differs from its manifest"));
        }
    }
    Ok(states)
}

pub(super) fn restore(layout: &Layout, progress: &impl ExtractProgress) -> Result<()> {
    let (dir, manifest) = load(layout)?;
    let states = verify(layout, &dir, &manifest, true, progress)?;
    cancelled(progress)?;
    write_json(&dir, "pending.json", &Pending::Restore)?;
    checkpoint("restore-journal");
    restore_checked(layout, dir, &manifest, states, &NonCancelling(progress))?;
    cleanup(layout)
}

pub(super) fn recover(layout: &Layout, progress: &impl ExtractProgress) -> Result<()> {
    if present(layout)? && pending(layout)? {
        let progress = NonCancelling(progress);
        let (dir, manifest) = load(layout)?;
        let states = verify(layout, &dir, &manifest, false, &progress)?;
        restore_checked(layout, dir, &manifest, states, &progress)?;
    }
    cleanup(layout)
}
fn restore_checked(
    layout: &Layout,
    dir: Dir,
    manifest: &MergeManifest,
    states: Vec<FileState>,
    progress: &impl ExtractProgress,
) -> Result<()> {
    let base = layout.base_dir()?;
    let incoming = checked_dir(&dir, Path::new("incoming"))?;
    let originals = checked_dir(&dir, Path::new("originals"))?;
    progress.phase("Restoring original game files");
    for (file, state) in manifest.files.iter().zip(states).rev() {
        if matches!(state, FileState::Published) {
            move_file(&base, &incoming, &file.after.path)?;
            checkpoint("restore-unpublish");
        }
        if !matches!(state, FileState::Original) && file.before.is_some() {
            move_file(&originals, &base, &file.after.path)?;
            checkpoint("restore-original");
        }
    }
    for path in manifest.created_directories.iter().rev() {
        if exists(&base, path)? {
            let parent = checked_dir(&base, Path::new(path).parent().unwrap())?;
            storage::require_directory(&parent, Path::new(path).file_name().unwrap())?;
            match parent.remove_dir(Path::new(path).file_name().unwrap()) {
                Ok(()) => sync_dir(&parent)?,
                Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    drop(originals);
    drop(incoming);
    drop(dir);
    let root = layout.manager()?;
    exclusive_move(root, ACTIVE.as_ref(), root, RETIRED.as_ref())?;
    if let Err(e) = sync_dir(root) {
        progress.warning(&format!("base restored, but final sync failed: {e}"));
    }
    checkpoint("restored");
    Ok(())
}

fn cleanup(layout: &Layout) -> Result<()> {
    let root = layout.manager()?;
    for path in [PREPARING, RETIRED] {
        if exists(root, path)? {
            storage::require_directory(root, path)?;
            root.remove_dir_all(path)?;
            sync_dir(root)?;
        }
    }
    Ok(())
}
fn cancelled(progress: &impl ExtractProgress) -> Result<()> {
    if progress.is_cancelled() {
        Err(ExtractError::Cancelled.into())
    } else {
        Ok(())
    }
}
struct NonCancelling<'a, P>(&'a P);
impl<P: ExtractProgress> ExtractProgress for NonCancelling<'_, P> {
    fn phase(&self, phase: &str) {
        self.0.phase(phase);
    }
    fn warning(&self, message: &str) {
        self.0.warning(message);
    }
    fn bytes_total(&self, bytes: u64) {
        self.0.bytes_total(bytes);
    }
    fn bytes_processed(&self, bytes: u64) {
        self.0.bytes_processed(bytes);
    }
}
fn checkpoint(_name: &str) {
    #[cfg(test)]
    FAIL_AT.with(|fail| {
        if fail.get() == Some(_name) {
            if FAIL_SKIP.with(|skip| {
                let n = skip.get();
                skip.set(n.saturating_sub(1));
                n > 0
            }) {
                return;
            }
            fail.set(None);
            panic!("merge crash at {_name}");
        }
    });
}
#[cfg(test)]
thread_local! { static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; static FAIL_SKIP: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
mod tests;
