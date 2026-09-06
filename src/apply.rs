//! Execute a plan: hooks, atomic writes, trash-backed removes, journal,
//! state update.

use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Module;
use crate::crypto::Crypto;
use crate::journal::{Journal, JournalEntry, move_file, now};
use crate::plan::{Direction, Op, Plan};
use crate::state::State;

#[derive(Debug, Default)]
pub struct Report {
    pub run: String,
    pub applied: usize,
    pub ran_before: bool,
    pub ran_after: bool,
}

pub struct Applier<'a> {
    pub crypto: &'a Crypto,
    pub journal: &'a Journal,
    pub state: &'a mut State,
    pub state_dir: &'a Path,
    pub run: String,
}

impl<'a> Applier<'a> {
    pub fn new(
        crypto: &'a Crypto,
        journal: &'a Journal,
        state: &'a mut State,
        state_dir: &'a Path,
    ) -> Applier<'a> {
        Applier {
            crypto,
            journal,
            state,
            state_dir,
            run: Journal::new_run_id(),
        }
    }

    pub fn apply(&mut self, module: &Module, plan: &Plan) -> Result<Report> {
        let mut report = Report {
            run: self.run.clone(),
            ..Default::default()
        };
        let view = module.view();

        if plan.first_run && module.hooks.has_before() {
            module
                .hooks
                .run_before(&view)
                .with_context(|| format!("module `{}`", module.name))?;
            report.ran_before = true;
        }

        for (i, op) in plan.ops.iter().enumerate() {
            self.apply_op(&module.name, i, op)
                .with_context(|| format!("{} {}", op.verb(), op.target().display()))?;
            report.applied += 1;
        }

        if plan.first_run && module.hooks.has_after() {
            module
                .hooks
                .run_after(&view)
                .with_context(|| format!("module `{}`", module.name))?;
            report.ran_after = true;
        }

        let entry = self.state.modules.entry(module.name.clone()).or_default();
        match plan.direction {
            Direction::Push => entry.last_push = Some(now()),
            Direction::Pull => entry.last_pull = Some(now()),
        }
        if plan.first_run
            && let Some(setup) = &module.setup
        {
            entry.setup_version = Some(setup.version);
            entry.setup_at = Some(now());
        }
        self.state.save_to(self.state_dir)?;
        Ok(report)
    }

    fn apply_op(&mut self, module: &str, index: usize, op: &Op) -> Result<()> {
        let index = index + 1;
        match op {
            Op::Copy { from, to } => {
                let bytes =
                    std::fs::read(from).with_context(|| format!("reading {}", from.display()))?;
                self.write(module, index, op, from, to, &bytes)
            }
            Op::Encrypt { from, to } => {
                let bytes =
                    std::fs::read(from).with_context(|| format!("reading {}", from.display()))?;
                let cipher = self.crypto.encrypt(&bytes)?;
                self.write(module, index, op, from, to, &cipher)
            }
            Op::Decrypt { from, to } => {
                let bytes =
                    std::fs::read(from).with_context(|| format!("reading {}", from.display()))?;
                let plain = self
                    .crypto
                    .decrypt(&bytes)
                    .with_context(|| format!("decrypting {}", from.display()))?;
                self.write(module, index, op, from, to, &plain)
            }
            Op::Remove { path } => {
                let trash = self.journal.trash(&self.run, module, index, path)?;
                self.journal.append(&JournalEntry {
                    run: self.run.clone(),
                    ts: now(),
                    module: module.to_owned(),
                    op: "remove".into(),
                    path: path.clone(),
                    from: None,
                    trash: Some(trash),
                    created: false,
                })
            }
        }
    }

    /// Write via a temp file in the same directory; the previous version, if
    /// any, goes to the trash first so the swap is a rename on every platform.
    fn write(
        &mut self,
        module: &str,
        index: usize,
        op: &Op,
        from: &Path,
        to: &Path,
        bytes: &[u8],
    ) -> Result<()> {
        let parent = to.parent().context("target has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        let tmp = parent.join(format!(
            ".qd-{}.tmp",
            to.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        ));
        std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        copy_permissions(from, &tmp);

        let existed = to.exists();
        let trash = if existed {
            Some(self.journal.trash(&self.run, module, index, to)?)
        } else {
            None
        };
        move_file(&tmp, to)?;

        self.journal.append(&JournalEntry {
            run: self.run.clone(),
            ts: now(),
            module: module.to_owned(),
            op: op.verb().into(),
            path: to.to_path_buf(),
            from: Some(from.to_path_buf()),
            trash,
            created: !existed,
        })
    }
}

#[cfg(unix)]
fn copy_permissions(from: &Path, to: &Path) {
    if let Ok(meta) = std::fs::metadata(from) {
        let _ = std::fs::set_permissions(to, meta.permissions());
    }
}

#[cfg(not(unix))]
fn copy_permissions(_from: &Path, _to: &Path) {}
