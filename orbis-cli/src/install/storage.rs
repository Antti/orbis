use super::*;
use crate::inventory::{FileRecord, InventoryDiff, compare_records, hash_file};
use cap_std::{ambient_authority, fs::OpenOptions};
use fs2::FileExt;
use serde::de::DeserializeOwned;
use std::{ffi::OsString, io::Read};

pub(super) const MANIFEST: &str = ".orbis-install.json";

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub schema_version: u32,
    pub title_id: String,
    pub content_id: String,
    pub app_version: String,
    pub files: Vec<FileRecord>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum RollbackTarget {
    #[default]
    None,
    Base,
    Update,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    pub schema_version: u32,
    pub active: bool,
    pub rollback: RollbackTarget,
}
impl Default for State {
    fn default() -> Self {
        Self {
            schema_version: 1,
            active: false,
            rollback: RollbackTarget::None,
        }
    }
}

pub(super) struct Layout {
    pub parent: Dir,
    pub parent_path: PathBuf,
    pub base: OsString,
    pub active: OsString,
    pub root_name: OsString,
    pub root: Option<Dir>,
    // Keep the OS lock until all directory handles and transaction work are done.
    _lock: Option<InstallLock>,
}

struct InstallLock(std::fs::File);

impl Drop for InstallLock {
    fn drop(&mut self) {
        // Closing our descriptor alone can leave flock held by a concurrently
        // forked child's copy until it execs. Explicitly release the lock when
        // this operation ends, regardless of those duplicate descriptors.
        let _ = FileExt::unlock(&self.0);
    }
}

impl Layout {
    pub fn open(base: &Path, write: bool) -> Result<Self> {
        Self::open_inner(base, write, false)
    }
    pub fn inspect(base: &Path) -> Result<Self> {
        Self::open_inner(base, false, true)
    }
    fn open_inner(base: &Path, write: bool, allow_legacy: bool) -> Result<Self> {
        let base_path = std::fs::canonicalize(base)?;
        if !base_path.is_dir() {
            return Err(invalid("base must be an extracted game directory"));
        }
        let name = base_path
            .file_name()
            .ok_or_else(|| invalid("base cannot be a filesystem root"))?;
        let parent_path = base_path.parent().unwrap().to_owned();
        let parent = Dir::open_ambient_dir(&parent_path, ambient_authority())?;
        let mut active = name.to_os_string();
        active.push("-UPDATE");
        let mut legacy = name.to_os_string();
        legacy.push("-patch");
        if !allow_legacy && exists(&parent, &legacy)? {
            return Err(invalid(
                "a sibling -patch directory already exists; resolve that update before using install",
            ));
        }
        let mut root_name = OsString::from(".");
        root_name.push(name);
        root_name.push(".orbis-install");
        if exists(&parent, &root_name)? {
            require_directory(&parent, &root_name)?;
        } else if write {
            parent.create_dir(&root_name)?;
            sync_dir(&parent)?;
        }
        let root = if exists(&parent, &root_name)? {
            Some(parent.open_dir(&root_name)?)
        } else {
            None
        };
        let lock = if write {
            let root = root.as_ref().unwrap();
            if exists(root, "lock")? {
                require_file(root, "lock")?;
            }
            let file = root
                .open_with(
                    "lock",
                    OpenOptions::new().create(true).read(true).write(true),
                )?
                .into_std();
            file.try_lock_exclusive()
                .map_err(|_| invalid("another installation operation holds the lock"))?;
            Some(InstallLock(file))
        } else {
            None
        };
        Ok(Self {
            parent,
            parent_path,
            base: name.to_owned(),
            active,
            root_name,
            root,
            _lock: lock,
        })
    }
    pub fn manager(&self) -> Result<&Dir> {
        self.root
            .as_ref()
            .ok_or_else(|| invalid("installation state directory is unavailable"))
    }
    pub fn active_path(&self) -> PathBuf {
        self.parent_path.join(&self.active)
    }
    pub fn stage_path(&self) -> PathBuf {
        self.parent_path.join(&self.root_name).join("staging")
    }
    pub fn base_dir(&self) -> Result<Dir> {
        require_directory(&self.parent, &self.base)?;
        Ok(self.parent.open_dir(&self.base)?)
    }
    pub fn base_metadata(&self) -> Result<AppMetadata> {
        if super::merge::present(self)? {
            return Err(invalid(
                "this game has a merged update; restore the base with rollback --base <DIR> before installing or exporting",
            ));
        }
        let result = app_metadata(&self.base_dir()?)?;
        if result.category != "gd" {
            return Err(invalid(
                "base PARAM.SFO CATEGORY must be gd; provide the original base directory",
            ));
        }
        Ok(result)
    }
    pub fn state(&self) -> Result<State> {
        let state = match &self.root {
            Some(root) if exists(root, "state.json")? => read_json(root, "state.json")?,
            _ => State::default(),
        };
        if state.schema_version != 1 || (!state.active && state.rollback != RollbackTarget::None) {
            return Err(invalid("unsupported or inconsistent installation state"));
        }
        Ok(state)
    }
    pub fn require_recovered(&self) -> Result<()> {
        if super::merge::pending(self)? {
            return Err(invalid(
                "an interrupted merge needs recovery; run recover --base <DIR>",
            ));
        }
        if self
            .root
            .as_ref()
            .is_some_and(|r| exists(r, "journal.json").unwrap_or(true))
        {
            return Err(invalid(
                "an interrupted installation needs recovery; run recover --base <DIR>",
            ));
        }
        Ok(())
    }
    pub fn active_manifest<P: ExtractProgress>(
        &self,
        state: &State,
        base: &AppMetadata,
        progress: &P,
    ) -> Result<Option<Manifest>> {
        if exists(&self.parent, &self.active)? != state.active {
            return Err(invalid(
                "unmanaged or missing -UPDATE directory; refusing to replace it",
            ));
        }
        if !state.active {
            return Ok(None);
        }
        require_directory(&self.parent, &self.active)?;
        let dir = self.parent.open_dir(&self.active)?;
        let manifest: Manifest = read_json(&dir, MANIFEST)?;
        progress.phase("Checking installed update");
        verify_manifest(&dir, &manifest, base, progress)?;
        Ok(Some(manifest))
    }
    pub fn verify_previous<P: ExtractProgress>(
        &self,
        state: &State,
        base: &AppMetadata,
        progress: &P,
    ) -> Result<Option<Manifest>> {
        let present = match &self.root {
            Some(root) => exists(root, "previous")?,
            None => false,
        };
        if present != (state.rollback == RollbackTarget::Update) {
            return Err(invalid("missing or unexpected rollback directory"));
        }
        if present {
            let root = self.manager()?;
            require_directory(root, "previous")?;
            let dir = root.open_dir("previous")?;
            let manifest: Manifest = read_json(&dir, MANIFEST)?;
            progress.phase("Checking rollback copy");
            verify_manifest(&dir, &manifest, base, progress)?;
            return Ok(Some(manifest));
        }
        Ok(None)
    }
    pub fn clean_uncommitted(&self) -> Result<()> {
        self.require_recovered()?;
        let root = self.manager()?;
        for name in ["staging", "retired"] {
            if exists(root, name)? {
                require_directory(root, name)?;
                root.remove_dir_all(name)?;
            }
        }
        Ok(())
    }
}

pub(super) fn exists(dir: &Dir, path: impl AsRef<Path>) -> Result<bool> {
    match dir.symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn require_directory(dir: &Dir, path: impl AsRef<Path>) -> Result<()> {
    if !dir.symlink_metadata(path)?.is_dir() {
        return Err(invalid("expected a directory, not a symlink or file"));
    }
    Ok(())
}
fn require_file(dir: &Dir, path: impl AsRef<Path>) -> Result<()> {
    if !dir.symlink_metadata(path)?.is_file() {
        return Err(invalid("expected a regular file, not a symlink"));
    }
    Ok(())
}
// Keep installation error variants and source chains stable at this boundary.
pub(super) fn read_json<T: DeserializeOwned>(dir: &Dir, name: &str) -> Result<T> {
    Ok(crate::inventory::read_json(dir, name)?)
}
pub(super) fn write_json(dir: &Dir, name: &str, value: &impl Serialize) -> Result<()> {
    Ok(crate::inventory::write_json(dir, name, value)?)
}
pub(super) fn sync_dir(dir: &Dir) -> Result<()> {
    Ok(crate::inventory::sync_dir(dir)?)
}
pub(super) fn app_metadata(dir: &Dir) -> Result<AppMetadata> {
    require_directory(dir, "sce_sys")?;
    let sys = dir.open_dir("sce_sys")?;
    require_file(&sys, "param.sfo")?;
    let mut bytes = Vec::new();
    sys.open("param.sfo")?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    Ok(AppMetadata::parse(&bytes)?)
}
pub(super) fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || (path.eq_ignore_ascii_case(MANIFEST)
            || path.eq_ignore_ascii_case(super::directory::MANIFEST))
    {
        return Err(invalid("package uses a reserved installation path"));
    }
    for part in path.split('/') {
        if !orbis_pkg::path::ComponentPolicy::Installation.accepts(part) {
            return Err(invalid(format!("unsupported installation path: {path}")));
        }
    }
    Ok(())
}

pub(super) fn inventory<P: ExtractProgress>(root: &Dir, progress: &P) -> Result<Vec<FileRecord>> {
    let mut records = Vec::new();
    // Queue paths, not open handles: a wide tree must not exhaust the process's
    // file descriptors before we get a chance to process its subdirectories.
    let mut pending = vec![String::new()];
    let mut buffer = vec![0; 1024 * 1024];
    while let Some(prefix) = pending.pop() {
        if progress.is_cancelled() {
            return Err(ExtractError::Cancelled.into());
        }
        let path = if prefix.is_empty() {
            "."
        } else {
            prefix.trim_end_matches('/')
        };
        require_directory(root, path)?;
        let dir = root.open_dir(path)?;
        for entry in dir.entries()? {
            if progress.is_cancelled() {
                return Err(ExtractError::Cancelled.into());
            }
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("non-UTF8 installation filename"))?;
            let path = format!("{prefix}{name}");
            if path == MANIFEST {
                continue;
            }
            validate_path(&path)?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(format!("{path}/"));
            } else if kind.is_file() {
                records.push(hash_file(root, &path, &mut buffer, progress)?);
            } else {
                return Err(invalid(
                    "symlinks and special files are not supported in managed updates",
                ));
            }
        }
    }
    records.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(records)
}
pub(super) fn verify_manifest<P: ExtractProgress>(
    dir: &Dir,
    manifest: &Manifest,
    base: &AppMetadata,
    progress: &P,
) -> Result<()> {
    validate_manifest_metadata(dir, manifest, base)?;
    if !compare_inventory(dir, manifest, progress)?.matches {
        return Err(invalid(
            "managed update files have changed; refusing to discard local modifications",
        ));
    }
    Ok(())
}

pub(super) fn validate_manifest_metadata(
    dir: &Dir,
    manifest: &Manifest,
    base: &AppMetadata,
) -> Result<()> {
    if manifest.schema_version != 1
        || manifest.title_id != base.title_id
        || manifest.content_id != base.content_id
    {
        return Err(invalid("update manifest does not match this base game"));
    }
    let meta = app_metadata(dir)?;
    if meta.category != "gp"
        || meta.content_id != manifest.content_id
        || meta.version.to_string() != manifest.app_version
    {
        return Err(invalid("installed update metadata has changed"));
    }
    Ok(())
}

pub(super) fn compare_inventory(
    dir: &Dir,
    manifest: &Manifest,
    progress: &impl ExtractProgress,
) -> Result<InventoryDiff> {
    let total = manifest
        .files
        .iter()
        .try_fold(0u64, |n, f| n.checked_add(f.bytes))
        .ok_or_else(|| invalid("manifest size overflows"))?;
    progress.bytes_total(total);
    let actual = inventory(dir, progress)?;
    Ok(compare_records(&manifest.files, &actual))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn dropping_layout_releases_lock_despite_a_duplicate_descriptor() {
        let temp = tempfile::tempdir().unwrap();
        let base = crate::test_support::base_game(temp.path());
        let layout = Layout::open(&base, true).unwrap();
        // A concurrent fork can retain this open file description until exec,
        // even when the descriptor is marked close-on-exec. Duplicate it here
        // to reproduce that lifetime deterministically without timing a fork.
        let inherited = layout._lock.as_ref().unwrap().0.try_clone().unwrap();
        assert!(Layout::open(&base, true).is_err());
        drop(layout);
        let next = Layout::open(&base, true).unwrap();
        assert!(Layout::open(&base, true).is_err());
        drop(inherited);
        assert!(Layout::open(&base, true).is_err());
        drop(next);
        assert!(Layout::open(&base, true).is_ok());
    }

    #[test]
    fn inventories_wide_trees_with_bounded_handles() {
        // Limit only a subprocess: changing this process's limit would interfere
        // with unrelated tests running on other threads.
        #[cfg(unix)]
        if std::env::var_os("ORBIS_TEST_INVENTORY_LIMIT").is_none() {
            let output = std::process::Command::new("sh")
                .args(["-c", "ulimit -n 256 && exec \"$@\"", "inventory-test"])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "install::storage::tests::inventories_wide_trees_with_bounded_handles",
                    "--nocapture",
                ])
                .env("ORBIS_TEST_INVENTORY_LIMIT", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let mut expected = Vec::new();
        for index in 0..300 {
            let directory = format!("directory-{index:03}/nested");
            std::fs::create_dir_all(temp.path().join(&directory)).unwrap();
            let path = format!("{directory}/file");
            std::fs::write(temp.path().join(&path), b"hello").unwrap();
            expected.push(FileRecord {
                path,
                bytes: 5,
                sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
            });
        }
        let dir = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        assert_eq!(inventory(&dir, &SilentProgress).unwrap(), expected);
    }

    #[test]
    fn oversized_state_is_refused_before_replacing_readable_state() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        write_json(&dir, "state.json", &State::default()).unwrap();
        let oversized = "x".repeat(crate::inventory::MAX_JSON as usize);
        assert!(write_json(&dir, "state.json", &oversized).is_err());
        let state: State = read_json(&dir, "state.json").unwrap();
        assert_eq!(state, State::default());
        assert_eq!(dir.entries().unwrap().count(), 1);
    }
}
