//! Load every config in a repository into resolved modules.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::{Module, RootConfig};
use crate::discover::discover;
use crate::host::Host;
use crate::lua::load_file;

pub struct Repo {
    pub root: PathBuf,
    pub host: Host,
    pub global: RootConfig,
    /// Enabled modules only, sorted by name.
    pub modules: Vec<Module>,
    /// Names of modules that returned `enabled = false` on this host.
    pub disabled: Vec<String>,
}

impl std::fmt::Debug for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repo")
            .field("root", &self.root)
            .field("modules", &self.modules)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl Repo {
    pub fn load(host: Host) -> Result<Repo> {
        let root = host.dotfiles.clone();
        let found = discover(&root)?;
        if found.root.is_none() && found.modules.is_empty() {
            bail!("{} contains no qd.lua files", root.display());
        }

        let global = match &found.root {
            Some(path) => {
                let loaded = load_file(&host, path)?;
                RootConfig::from_raw(loaded.raw, &root)
                    .with_context(|| format!("{}", path.display()))?
            }
            None => RootConfig::default(),
        };

        let mut modules = Vec::new();
        let mut disabled = Vec::new();
        for (name, path) in found.modules {
            let src = path.parent().expect("qd.lua has a parent").to_path_buf();
            let loaded = load_file(&host, &path)?;
            let module =
                Module::resolve(&host, &global, name.clone(), src, loaded.raw, loaded.hooks)?;
            if module.enabled {
                modules.push(module);
            } else {
                disabled.push(name);
            }
        }

        Ok(Repo {
            root,
            host,
            global,
            modules,
            disabled,
        })
    }

    pub fn module(&self, name: &str) -> Result<&Module> {
        self.modules
            .iter()
            .find(|m| m.name == name)
            .with_context(|| {
                if self.disabled.iter().any(|d| d == name) {
                    format!("module `{name}` is disabled on this host")
                } else {
                    format!("no module named `{name}`")
                }
            })
    }

    /// Modules that have a `path` and therefore take part in push/pull.
    pub fn syncable(&self) -> impl Iterator<Item = &Module> {
        self.modules.iter().filter(|m| m.dest.is_some())
    }
}

/// Resolve the repository root from, in order: an explicit flag, `QD_REPO`,
/// the state file, then the current directory if it looks like a repo.
pub fn find_root(explicit: Option<&Path>, from_state: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return canonical(p);
    }
    if let Some(p) = std::env::var_os("QD_REPO") {
        return canonical(Path::new(&p));
    }
    if let Some(p) = from_state {
        return canonical(p);
    }
    let cwd = std::env::current_dir()?;
    if crate::discover::looks_like_repo(&cwd) {
        return Ok(cwd);
    }
    bail!("cannot locate the dotfiles repo: pass --repo, set QD_REPO, or run from inside it")
}

fn canonical(p: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(p).with_context(|| format!("repo path {} does not exist", p.display()))
}
