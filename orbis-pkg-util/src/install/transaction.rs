use super::*;
use storage::{RollbackTarget, State, exists, require_directory, sync_dir, write_json};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Install,
    Rollback,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    operation: Operation,
    before: State,
}

impl Layout {
    fn journal(&self, operation: Operation) -> Result<State> {
        let before = self.state()?;
        write_json(
            self.manager()?,
            "journal.json",
            &Journal {
                schema_version: 1,
                operation,
                before: before.clone(),
            },
        )?;
        checkpoint("journal")?;
        Ok(before)
    }
    fn sync_renames(&self) -> Result<()> {
        sync_dir(self.manager()?)?;
        sync_dir(&self.parent)
    }
    fn finish_journal<P: ExtractProgress>(&self, progress: &P) -> Result<()> {
        let root = self.manager()?;
        root.remove_file("journal.json")?;
        // Journal removal commits the operation. A failed durability sync cannot
        // undo it, so report the warning without taking the pre-commit error path.
        if let Err(error) = checkpoint("final_sync").and_then(|()| sync_dir(root)) {
            let cause = std::error::Error::source(&error).unwrap_or(&error);
            progress.warning(&format!(
                "Operation committed, but final directory sync failed: {cause}. \
                 Power-loss durability is uncertain."
            ));
        }
        Ok(())
    }
    fn commit<P: ExtractProgress>(&self, state: &State, progress: &P) -> Result<()> {
        let root = self.manager()?;
        write_json(root, "state.json", state)?;
        checkpoint("state")?;
        self.finish_journal(progress)?;
        let _ = checkpoint("committed");
        // Old backups can be large; cleanup can be retried by recover. Once the
        // journal is removed, installation has committed and must report success.
        let _ = self.clean_uncommitted();
        Ok(())
    }
    pub fn activate<P: ExtractProgress>(&self, progress: &P) -> Result<()> {
        let root = self.manager()?;
        let before = self.journal(Operation::Install)?;
        if exists(root, "previous")? {
            require_directory(root, "previous")?;
            root.rename("previous", root, "retired")?;
            self.sync_renames()?;
        }
        checkpoint("retired")?;
        if before.active {
            self.parent.rename(&self.active, root, "previous")?;
            self.sync_renames()?;
        }
        checkpoint("previous")?;
        root.rename("staging", &self.parent, &self.active)?;
        self.sync_renames()?;
        checkpoint("activated")?;
        self.commit(
            &State {
                schema_version: 1,
                active: true,
                rollback: if before.active {
                    RollbackTarget::Update
                } else {
                    RollbackTarget::Base
                },
            },
            progress,
        )
    }
    pub fn rollback_transaction<P: ExtractProgress>(&self, progress: &P) -> Result<()> {
        let root = self.manager()?;
        let before = self.journal(Operation::Rollback)?;
        self.parent.rename(&self.active, root, "retired")?;
        self.sync_renames()?;
        checkpoint("retired")?;
        let active = before.rollback == RollbackTarget::Update;
        if active {
            root.rename("previous", &self.parent, &self.active)?;
            self.sync_renames()?;
        }
        checkpoint("activated")?;
        self.commit(
            &State {
                schema_version: 1,
                active,
                rollback: RollbackTarget::None,
            },
            progress,
        )
    }
    pub fn recover_transaction<P: ExtractProgress>(&self, progress: &P) -> Result<()> {
        let root = self.manager()?;
        if !exists(root, "journal.json")? {
            return Ok(());
        }
        let journal: Journal = read_json(root, "journal.json")?;
        if journal.schema_version != 1
            || journal.before.schema_version != 1
            || (!journal.before.active && journal.before.rollback != RollbackTarget::None)
            || (matches!(journal.operation, Operation::Rollback)
                && (!journal.before.active || journal.before.rollback == RollbackTarget::None))
        {
            return Err(invalid(
                "unsupported recovery journal; no directories were changed",
            ));
        }
        for name in ["staging", "previous", "retired"] {
            if exists(root, name)? {
                require_directory(root, name)?;
            }
        }
        if exists(&self.parent, &self.active)? {
            require_directory(&self.parent, &self.active)?;
        }
        match journal.operation {
            Operation::Install => {
                // Staging disappears only when it has become the active update.
                if !exists(root, "staging")? {
                    if !exists(&self.parent, &self.active)? {
                        return Err(invalid("recovery cannot locate staged or active update"));
                    }
                    self.parent.rename(&self.active, root, "staging")?;
                    self.sync_renames()?;
                }
                if journal.before.active && !exists(&self.parent, &self.active)? {
                    root.rename("previous", &self.parent, &self.active)?;
                    self.sync_renames()?;
                }
                if exists(root, "retired")? {
                    if exists(root, "previous")? {
                        return Err(invalid("recovery found conflicting backup directories"));
                    }
                    root.rename("retired", root, "previous")?;
                    self.sync_renames()?;
                }
            }
            Operation::Rollback => {
                if exists(root, "retired")? {
                    if exists(&self.parent, &self.active)? {
                        if exists(root, "previous")? {
                            return Err(invalid("recovery found conflicting active directories"));
                        }
                        self.parent.rename(&self.active, root, "previous")?;
                        self.sync_renames()?;
                    }
                    root.rename("retired", &self.parent, &self.active)?;
                    self.sync_renames()?;
                }
            }
        }
        write_json(root, "state.json", &journal.before)?;
        self.finish_journal(progress)
    }
    pub fn restore_after_error<P: ExtractProgress>(
        &self,
        error: &InstallError,
        progress: &P,
    ) -> Result<()> {
        self.recover_transaction(progress).and_then(|()| self.clean_uncommitted()).map_err(|recovery| invalid(format!(
            "{error}; automatic recovery failed: {recovery}; run recover --base <DIR> before retrying"
        )))
    }
}

// Test-only process interruption at persisted transaction boundaries.
fn checkpoint(_name: &'static str) -> Result<()> {
    #[cfg(test)]
    FAIL_AT.with(|fault| {
        assert!(
            fault.get() != Some(_name),
            "simulated process exit at {_name}"
        );
    });
    #[cfg(test)]
    if ERROR_AT.with(|fault| fault.get() == Some(_name)) {
        return Err(std::io::Error::other("simulated write failure").into());
    }
    Ok(())
}
#[cfg(test)]
thread_local! {
    pub(super) static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    pub(super) static ERROR_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}
