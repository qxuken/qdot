//! Turn one module plus the current disk state into a list of operations.
//! Pure: reads files, writes nothing. `status` and `--dry-run` print the plan;
//! `apply` executes it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::Module;
use crate::crypto::Crypto;
use crate::scan::{AGE_EXT, Entry, scan_dest, scan_src, to_slash};
use crate::state::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// repo → machine
    Push,
    /// machine → repo
    Pull,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    Copy { from: PathBuf, to: PathBuf },
    Encrypt { from: PathBuf, to: PathBuf },
    Decrypt { from: PathBuf, to: PathBuf },
    Remove { path: PathBuf },
}

impl Op {
    pub fn target(&self) -> &Path {
        match self {
            Op::Copy { to, .. } | Op::Encrypt { to, .. } | Op::Decrypt { to, .. } => to,
            Op::Remove { path } => path,
        }
    }

    pub fn is_remove(&self) -> bool {
        matches!(self, Op::Remove { .. })
    }

    pub fn verb(&self) -> &'static str {
        match self {
            Op::Copy { .. } => "copy",
            Op::Encrypt { .. } => "encrypt",
            Op::Decrypt { .. } => "decrypt",
            Op::Remove { .. } => "remove",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub module: String,
    pub direction: Direction,
    pub dest: PathBuf,
    /// Setup hooks will run: dest missing, no state, version bump, or --force.
    pub first_run: bool,
    pub first_run_reason: Option<String>,
    pub ops: Vec<Op>,
    /// Removes that `--no-remove` held back.
    pub skipped_removes: Vec<PathBuf>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty() && !self.first_run
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PlanOpts {
    pub force: bool,
    pub no_remove: bool,
    /// Never run setup hooks (and do not record a setup version).
    pub no_setup: bool,
}

pub fn plan(
    module: &Module,
    direction: Direction,
    crypto: &Crypto,
    state: &State,
    opts: PlanOpts,
) -> Result<Plan> {
    let dest = module
        .dest
        .clone()
        .with_context(|| format!("module `{}` has no `path`", module.name))?;
    let mut src_tree = scan_src(&module.src, &module.ignore_set)?;
    let mut dest_tree = scan_dest(&dest, &module.ignore_set)?;

    // A file named as a `files[].src` is owned by that pair and is not mirrored
    // into the module directory as well.
    for pair in &module.files {
        if let Ok(rel) = pair.src.strip_prefix(&module.src) {
            let rel = to_slash(rel);
            src_tree.remove(&rel);
            dest_tree.remove(&rel);
        }
    }

    let mut ops = Vec::new();
    let mut removes = Vec::new();

    match direction {
        Direction::Push => {
            for (rel, s) in &src_tree {
                let to = dest.join(rel);
                let changed = match dest_tree.get(rel) {
                    None => true,
                    Some(d) => !same_content(s, d, crypto)?,
                };
                if changed {
                    ops.push(if s.encrypted {
                        Op::Decrypt {
                            from: s.abs.clone(),
                            to,
                        }
                    } else {
                        Op::Copy {
                            from: s.abs.clone(),
                            to,
                        }
                    });
                }
            }
            for (rel, d) in &dest_tree {
                if !src_tree.contains_key(rel) {
                    removes.push(d.abs.clone());
                }
            }
            for pair in &module.files {
                if let Some(repo) = repo_entry(&pair.src) {
                    let changed = match plain_entry(&pair.dest) {
                        None => true,
                        Some(d) => !same_content(&repo, &d, crypto)?,
                    };
                    if changed {
                        ops.push(if repo.encrypted {
                            Op::Decrypt {
                                from: repo.abs,
                                to: pair.dest.clone(),
                            }
                        } else {
                            Op::Copy {
                                from: repo.abs,
                                to: pair.dest.clone(),
                            }
                        });
                    }
                }
            }
            for inc in &module.include {
                let Some(name) = inc.file_name() else {
                    continue;
                };
                let to = dest.join(name);
                if !inc.is_file() {
                    continue;
                }
                let changed = match plain_entry(&to) {
                    None => true,
                    Some(d) => !same_content(&plain(inc), &d, crypto)?,
                };
                if changed {
                    ops.push(Op::Copy {
                        from: inc.clone(),
                        to,
                    });
                }
            }
        }
        Direction::Pull => {
            for (rel, d) in &dest_tree {
                let want_encrypted = module.encrypt_set.is_match(rel);
                let to = repo_path(&module.src, rel, want_encrypted);
                let (changed, stale) = match src_tree.get(rel) {
                    None => (true, None),
                    Some(s) if s.encrypted != want_encrypted => (true, Some(s.abs.clone())),
                    Some(s) => (!same_content(s, d, crypto)?, None),
                };
                if changed {
                    ops.push(if want_encrypted {
                        Op::Encrypt {
                            from: d.abs.clone(),
                            to,
                        }
                    } else {
                        Op::Copy {
                            from: d.abs.clone(),
                            to,
                        }
                    });
                }
                if let Some(old) = stale {
                    ops.push(Op::Remove { path: old });
                }
            }
            for (rel, s) in &src_tree {
                if !dest_tree.contains_key(rel) {
                    removes.push(s.abs.clone());
                }
            }
            for pair in &module.files {
                let Some(d) = plain_entry(&pair.dest) else {
                    continue;
                };
                let rel = pair
                    .src
                    .strip_prefix(&module.src)
                    .map(to_slash)
                    .unwrap_or_default();
                let existing = repo_entry(&pair.src);
                let encrypted = existing
                    .as_ref()
                    .map(|e| e.encrypted)
                    .unwrap_or_else(|| module.encrypt_set.is_match(&rel));
                let changed = match &existing {
                    None => true,
                    Some(s) => !same_content(s, &d, crypto)?,
                };
                if changed {
                    let to = with_age(&pair.src, encrypted);
                    ops.push(if encrypted {
                        Op::Encrypt { from: d.abs, to }
                    } else {
                        Op::Copy { from: d.abs, to }
                    });
                }
            }
        }
    }

    let mut skipped_removes = Vec::new();
    if opts.no_remove {
        skipped_removes = removes;
    } else {
        ops.extend(removes.into_iter().map(|path| Op::Remove { path }));
    }

    let (first_run, first_run_reason) = match direction {
        Direction::Push => first_run(module, &dest, state, opts.force, opts.no_setup),
        Direction::Pull => (false, None),
    };

    Ok(Plan {
        module: module.name.clone(),
        direction,
        dest,
        first_run,
        first_run_reason,
        ops,
        skipped_removes,
    })
}

fn first_run(
    module: &Module,
    dest: &Path,
    state: &State,
    force: bool,
    no_setup: bool,
) -> (bool, Option<String>) {
    let Some(setup) = &module.setup else {
        return (false, None);
    };
    if no_setup {
        return (false, None);
    }
    if force {
        return (true, Some("--force".into()));
    }
    if !dest.exists() {
        return (true, Some("destination does not exist".into()));
    }
    match state
        .modules
        .get(&module.name)
        .and_then(|m| m.setup_version)
    {
        None => (true, Some("no setup recorded on this machine".into())),
        Some(v) if v < setup.version => {
            (true, Some(format!("setup version {v} → {}", setup.version)))
        }
        Some(_) => (false, None),
    }
}

/// Content of a repo file, or `<file>.age` when only that exists.
fn repo_entry(path: &Path) -> Option<Entry> {
    if path.is_file() {
        return Some(plain(path));
    }
    let aged = with_age(path, true);
    aged.is_file().then_some(Entry {
        abs: aged,
        encrypted: true,
    })
}

fn plain_entry(path: &Path) -> Option<Entry> {
    path.is_file().then(|| plain(path))
}

fn plain(path: &Path) -> Entry {
    Entry {
        abs: path.to_path_buf(),
        encrypted: false,
    }
}

fn repo_path(src: &Path, rel: &str, encrypted: bool) -> PathBuf {
    with_age(&src.join(rel), encrypted)
}

fn with_age(path: &Path, encrypted: bool) -> PathBuf {
    if !encrypted {
        return path.to_path_buf();
    }
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{AGE_EXT}"));
    PathBuf::from(s)
}

/// Compare plaintext bytes. Encrypted entries are decrypted in memory, since
/// age output is randomized and ciphertext never compares equal.
fn same_content(a: &Entry, b: &Entry, crypto: &Crypto) -> Result<bool> {
    let a = plaintext(a, crypto)?;
    let b = plaintext(b, crypto)?;
    Ok(a == b)
}

pub fn plaintext(e: &Entry, crypto: &Crypto) -> Result<Vec<u8>> {
    let bytes = std::fs::read(&e.abs).with_context(|| format!("reading {}", e.abs.display()))?;
    if e.encrypted {
        crypto
            .decrypt(&bytes)
            .with_context(|| format!("decrypting {}", e.abs.display()))
    } else {
        Ok(bytes)
    }
}
