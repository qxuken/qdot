//! Find config files in the repository: an optional root `qd.lua` and one
//! `qd.lua` per direct subdirectory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const CONFIG_FILE: &str = "qd.lua";

#[derive(Debug, Default)]
pub struct Discovered {
    pub root: Option<PathBuf>,
    /// `(module name, path to its qd.lua)`, sorted by name.
    pub modules: Vec<(String, PathBuf)>,
}

pub fn discover(repo: &Path) -> Result<Discovered> {
    let mut out = Discovered::default();
    let root = repo.join(CONFIG_FILE);
    if root.is_file() {
        out.root = Some(root);
    }
    for entry in std::fs::read_dir(repo).with_context(|| format!("reading {}", repo.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let cfg = entry.path().join(CONFIG_FILE);
        if cfg.is_file() {
            out.modules
                .push((entry.file_name().to_string_lossy().into_owned(), cfg));
        }
    }
    out.modules.sort();
    Ok(out)
}

/// True if `dir` looks like a dotfiles repo: has a root qd.lua or any module.
pub fn looks_like_repo(dir: &Path) -> bool {
    discover(dir)
        .map(|d| d.root.is_some() || !d.modules.is_empty())
        .unwrap_or(false)
}
