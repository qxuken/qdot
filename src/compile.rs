//! Run every plugin's `compile` step and put the results on disk the same way
//! `apply` does: atomic writes, previous content to the trash, one journal
//! entry per file, all under the caller's run id so `undo` covers them.
//!
//! With the built-in plugins this regenerates `~/.dotfiles.local.nu` and
//! `~/.dotfiles-env.local.nu`.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::journal::{Journal, JournalEntry, move_file, now};
use crate::plugin::Output;
use crate::repo::Repo;

/// Every file the plugins want, in plugin order. Pure.
pub fn compile(repo: &Repo) -> Result<Vec<Output>> {
    let ctx = repo.ctx();
    let mut outputs = Vec::new();
    for plugin in repo.plugins.iter() {
        outputs.extend(plugin.compile(&ctx)?);
    }
    let mut owners: HashMap<&PathBuf, &str> = HashMap::new();
    for o in &outputs {
        if let Some(other) = owners.insert(&o.path, &o.plugin) {
            bail!(
                "plugins `{other}` and `{}` both compile {}",
                o.plugin,
                o.path.display()
            );
        }
    }
    Ok(outputs)
}

/// The subset of `outputs` whose content differs from what is on disk.
pub fn changed(outputs: &[Output]) -> Vec<&Output> {
    outputs
        .iter()
        .filter(|o| {
            std::fs::read(&o.path)
                .map(|bytes| bytes != o.content.as_bytes())
                .unwrap_or(true)
        })
        .collect()
}

/// Compile and write whatever changed. Returns the paths written.
pub fn apply(repo: &Repo, journal: &Journal, run: &str) -> Result<Vec<PathBuf>> {
    let outputs = compile(repo)?;
    let mut written = Vec::new();
    for (i, o) in changed(&outputs).into_iter().enumerate() {
        write(journal, run, i + 1, o).with_context(|| format!("compiling {}", o.path.display()))?;
        written.push(o.path.clone());
    }
    Ok(written)
}

fn write(journal: &Journal, run: &str, index: usize, o: &Output) -> Result<()> {
    let parent = o.path.parent().context("target has no parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let tmp = parent.join(format!(
        ".qd-{}.tmp",
        o.path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, &o.content).with_context(|| format!("writing {}", tmp.display()))?;

    let existed = o.path.exists();
    let trash = if existed {
        Some(journal.trash(run, &o.plugin, index, &o.path)?)
    } else {
        None
    };
    move_file(&tmp, &o.path)?;

    journal.append(&JournalEntry {
        run: run.to_owned(),
        ts: now(),
        module: o.plugin.clone(),
        op: "compile".into(),
        path: o.path.clone(),
        from: None,
        trash,
        created: !existed,
    })
}
