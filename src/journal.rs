//! Append-only journal of applied operations plus a trash directory holding
//! every file that was overwritten or removed. Together they make `undo`
//! possible without a database.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const JOURNAL_FILE: &str = "journal.jsonl";
pub const TRASH_DIR: &str = "trash";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub run: String,
    pub ts: String,
    pub module: String,
    pub op: String,
    /// The file that was written or removed.
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<PathBuf>,
    /// Where the previous content of `path` went, if there was any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trash: Option<PathBuf>,
    /// `path` did not exist before this op.
    #[serde(default)]
    pub created: bool,
}

pub struct Journal {
    dir: PathBuf,
}

impl Journal {
    pub fn new(state_dir: &Path) -> Journal {
        Journal {
            dir: state_dir.to_path_buf(),
        }
    }

    pub fn file(&self) -> PathBuf {
        self.dir.join(JOURNAL_FILE)
    }

    pub fn trash_root(&self) -> PathBuf {
        self.dir.join(TRASH_DIR)
    }

    pub fn new_run_id() -> String {
        format!(
            "{}-{}",
            jiff::Timestamp::now().as_millisecond(),
            std::process::id()
        )
    }

    pub fn append(&self, entry: &JournalEntry) -> Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir)?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.file())?;
        serde_json::to_writer(&mut f, entry)?;
        f.write_all(b"\n")?;
        Ok(())
    }

    pub fn read_all(&self) -> Result<Vec<JournalEntry>> {
        let file = self.file();
        if !file.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&file)?;
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).with_context(|| format!("bad journal line: {l}")))
            .collect()
    }

    /// Move `path` into the trash for `run`; returns the new location.
    /// Keyed by `module` and op `index`, so two modules trashing a file with
    /// the same name at the same index never overwrite each other.
    pub fn trash(&self, run: &str, module: &str, index: usize, path: &Path) -> Result<PathBuf> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let dir = self
            .trash_root()
            .join(run)
            .join(module)
            .join(index.to_string());
        std::fs::create_dir_all(&dir)?;
        let target = dir.join(name);
        move_file(path, &target)?;
        Ok(target)
    }

    /// Ids of runs recorded in the journal, oldest first.
    pub fn runs(&self) -> Result<Vec<String>> {
        let mut runs: Vec<String> = Vec::new();
        for e in self.read_all()? {
            if runs.last() != Some(&e.run) {
                runs.push(e.run);
            }
        }
        Ok(runs)
    }

    /// Revert every op of `run` in reverse order: restore trashed content,
    /// delete files the run created. Records the undo as its own run.
    pub fn undo(&self, run: &str) -> Result<Vec<JournalEntry>> {
        let entries: Vec<JournalEntry> = self
            .read_all()?
            .into_iter()
            .filter(|e| e.run == run)
            .collect();
        if entries.is_empty() {
            bail!("no journal entries for run {run}");
        }
        let undo_run = format!("undo-{}", Journal::new_run_id());
        let mut done = Vec::new();
        for (i, e) in entries.iter().rev().enumerate() {
            match (&e.trash, e.created) {
                (Some(trash), _) => {
                    let mut trashed_current = None;
                    if e.path.exists() {
                        trashed_current = Some(self.trash(&undo_run, &e.module, i, &e.path)?);
                    }
                    if let Some(parent) = e.path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    move_file(trash, &e.path)?;
                    let rec = JournalEntry {
                        run: undo_run.clone(),
                        ts: now(),
                        module: e.module.clone(),
                        op: "restore".into(),
                        path: e.path.clone(),
                        from: Some(trash.clone()),
                        trash: trashed_current,
                        created: false,
                    };
                    self.append(&rec)?;
                    done.push(rec);
                }
                (None, true) => {
                    if e.path.exists() {
                        let t = self.trash(&undo_run, &e.module, i, &e.path)?;
                        let rec = JournalEntry {
                            run: undo_run.clone(),
                            ts: now(),
                            module: e.module.clone(),
                            op: "remove".into(),
                            path: e.path.clone(),
                            from: None,
                            trash: Some(t),
                            created: false,
                        };
                        self.append(&rec)?;
                        done.push(rec);
                    }
                }
                (None, false) => {}
            }
        }
        Ok(done)
    }

    /// Delete trash for runs older than `max_age`. Run ids start with a
    /// millisecond timestamp, so no metadata is needed.
    pub fn prune_trash(&self, max_age: jiff::SignedDuration) -> Result<Vec<PathBuf>> {
        let root = self.trash_root();
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let cutoff = jiff::Timestamp::now()
            .checked_sub(max_age)?
            .as_millisecond();
        let mut removed = Vec::new();
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let ms = name
                .trim_start_matches("undo-")
                .split('-')
                .next()
                .and_then(|s| s.parse::<i64>().ok());
            if let Some(ms) = ms
                && ms < cutoff
            {
                std::fs::remove_dir_all(entry.path())?;
                removed.push(entry.path());
            }
        }
        Ok(removed)
    }
}

pub fn now() -> String {
    jiff::Timestamp::now()
        .round(jiff::Unit::Second)
        .unwrap_or_else(|_| jiff::Timestamp::now())
        .to_string()
}

/// Rename, falling back to copy+delete across filesystems.
pub fn move_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if to.exists() {
        std::fs::remove_file(to)?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)
                .with_context(|| format!("moving {} to {}", from.display(), to.display()))?;
            std::fs::remove_file(from)?;
            Ok(())
        }
    }
}
