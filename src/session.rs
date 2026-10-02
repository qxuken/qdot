//! qd as a library: a [`Session`] is the machine's state and the repo it
//! points at, and every operation the CLI has, returning data and
//! printing nothing. `cli.rs` is a printer over it, so a program that
//! links qd (an editor's dotfiles pane) runs the same code the `qd`
//! binary does on the same `state.toml`, journal and trash.
//!
//! ```no_run
//! let mut s = qd::Session::open(None)?;
//! for plan in s.status(&[], qd::plan::Direction::Push)? {
//!     println!("{}: {} ops", plan.module, plan.ops.len());
//! }
//! let done = s.sync(&["helix".into()], qd::plan::Direction::Pull, qd::SyncOpts::default())?;
//! println!("applied {} (run {})", done.applied, done.run);
//! # anyhow::Ok(())
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::apply::Applier;
use crate::compile;
use crate::config::Module;
use crate::crypto::{Crypto, IDENTITY_FILE, RECIPIENTS_FILE};
use crate::host::Host;
use crate::journal::Journal;
use crate::plan::{Direction, Plan, PlanOpts, plan};
use crate::repo::{Repo, find_root};
use crate::state::{State, state_dir};

/// The version of qd this is: what a program linking it compares with
/// the `qd` it finds on the PATH, the two sharing one state.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The machine's state and the repo it points at.
pub struct Session {
    pub state_dir: PathBuf,
    pub state: State,
    /// `--repo`: the repo named outright, over `QD_REPO` and the state's.
    pub repo_flag: Option<PathBuf>,
}

/// How a push or a pull goes.
#[derive(Clone, Copy, Debug, Default)]
pub struct SyncOpts {
    /// Removes held back.
    pub no_remove: bool,
    /// The plugins' compile step skipped.
    pub no_compile: bool,
    /// Push: setup hooks run as on a first run.
    pub force: bool,
    /// Push: setup hooks not run.
    pub no_setup: bool,
}

/// What a push or a pull did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Synced {
    /// The run's id, for `undo`; empty when nothing was applied.
    pub run: String,
    pub applied: usize,
    /// The files the compile step wrote.
    pub compiled: Vec<PathBuf>,
}

impl Session {
    /// The state from `QD_STATE` or the platform's state directory, and
    /// `repo` over the one it records.
    pub fn open(repo: Option<PathBuf>) -> Result<Session> {
        let state_dir = state_dir()?;
        let state = State::load_from(&state_dir)?;
        Ok(Session {
            state_dir,
            state,
            repo_flag: repo,
        })
    }

    /// The repo's root: `--repo`, `QD_REPO`, the state's, or the working
    /// directory when it looks like one.
    pub fn root(&self) -> Result<PathBuf> {
        find_root(
            self.repo_flag.as_deref(),
            self.state.machine.repo.as_deref(),
        )
    }

    pub fn host(&self) -> Result<Host> {
        Host::detect(self.root()?, self.state.machine.tags.iter().cloned())
    }

    /// The repo, every module's `qd.lua` loaded.
    pub fn repo(&self) -> Result<Repo> {
        Repo::load(self.host()?)
    }

    pub fn crypto(&self, root: &Path) -> Result<Crypto> {
        let identity = self
            .state
            .machine
            .identity
            .clone()
            .unwrap_or_else(|| root.join(IDENTITY_FILE));
        Crypto::load(&root.join(RECIPIENTS_FILE), Some(&identity))
    }

    pub fn journal(&self) -> Journal {
        Journal::new(&self.state_dir)
    }

    pub fn save(&self) -> Result<()> {
        self.state.save_to(&self.state_dir)
    }

    /// What a push (or a pull) of `modules` — every syncable one when
    /// empty — would do.
    pub fn status(&self, modules: &[String], direction: Direction) -> Result<Vec<Plan>> {
        let repo = self.repo()?;
        let crypto = self.crypto(&repo.root)?;
        Ok(plan_modules(
            &repo,
            modules,
            direction,
            &crypto,
            &self.state,
            PlanOpts::default(),
        )?
        .into_iter()
        .map(|(_, p)| p)
        .collect())
    }

    /// Pushes or pulls `modules` — every syncable one when empty — and
    /// compiles, all under one run.
    pub fn sync(
        &mut self,
        modules: &[String],
        direction: Direction,
        opts: SyncOpts,
    ) -> Result<Synced> {
        let repo = self.repo()?;
        let crypto = self.crypto(&repo.root)?;
        let plans = plan_modules(
            &repo,
            modules,
            direction,
            &crypto,
            &self.state,
            self.plan_opts(direction, opts),
        )?;
        let (run, applied) = self.apply(&crypto, &plans)?;
        let compiled = if opts.no_compile {
            Vec::new()
        } else {
            self.compile(&repo, &run)?
        };
        Ok(Synced {
            run,
            applied,
            compiled,
        })
    }

    pub(crate) fn plan_opts(&self, direction: Direction, opts: SyncOpts) -> PlanOpts {
        match direction {
            Direction::Push => PlanOpts {
                force: opts.force,
                no_remove: opts.no_remove,
                no_setup: opts.no_setup,
            },
            Direction::Pull => PlanOpts {
                force: false,
                no_remove: opts.no_remove,
                no_setup: false,
            },
        }
    }

    /// Applies every plan under one run id and returns it with how many
    /// operations were applied, so the compile step joins the same run
    /// and `undo` reverts both together.
    pub fn apply(&mut self, crypto: &Crypto, plans: &[(&Module, Plan)]) -> Result<(String, usize)> {
        let journal = self.journal();
        let state_dir = self.state_dir.clone();
        let mut applier = Applier::new(crypto, &journal, &mut self.state, &state_dir);
        let mut total = 0;
        for (m, p) in plans {
            if p.is_empty() {
                continue;
            }
            total += applier.apply(m, p)?.applied;
        }
        Ok((applier.run, total))
    }

    /// Writes every plugin's compile output under `run`.
    pub fn compile(&self, repo: &Repo, run: &str) -> Result<Vec<PathBuf>> {
        compile::apply(repo, &self.journal(), run)
    }

    /// Starts keeping `dest` as module `name`: `NAME/qd.lua` written in
    /// the repo with `path` — under the home as `qd.path.home(…)`, which
    /// reads the same on every machine — and `ignore` globs. The files
    /// come in with a pull.
    pub fn add_module(&self, name: &str, dest: &Path, ignore: &[String]) -> Result<PathBuf> {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            bail!("a module's name is letters, digits, `-`, `_` and `.`: `{name}`");
        }
        if !dest.is_dir() {
            bail!("{} is not a directory", dest.display());
        }
        let root = self.root()?;
        let dir = root.join(name);
        let file = dir.join("qd.lua");
        if file.exists() {
            bail!("{} exists already", file.display());
        }
        // The home as written and as it resolves: `dest` may have come
        // through `canonicalize` (`/var` → `/private/var` on macOS).
        let home = dirs::home_dir();
        let homes: Vec<PathBuf> = home
            .iter()
            .flat_map(|h| [Some(h.clone()), crate::path::canonicalize(h).ok()])
            .flatten()
            .collect();
        let text = module_file(dest, &homes, ignore);
        std::fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
        std::fs::write(&file, text).with_context(|| file.display().to_string())?;
        Ok(file)
    }
}

/// A module's `qd.lua` for `dest`.
fn module_file(dest: &Path, homes: &[PathBuf], ignore: &[String]) -> String {
    let lua = |s: &str| format!("{s:?}");
    let path = match homes.iter().find_map(|h| dest.strip_prefix(h).ok()) {
        Some(rel) if rel.components().count() > 0 => {
            let parts: Vec<String> = rel
                .components()
                .map(|c| lua(&c.as_os_str().to_string_lossy()))
                .collect();
            format!("qd.path.home({})", parts.join(", "))
        }
        _ => lua(&dest.to_string_lossy()),
    };
    let mut out = format!("local qd = require(\"qd\")\n\nreturn {{\n  path = {path},\n");
    if !ignore.is_empty() {
        let globs: Vec<String> = ignore.iter().map(|g| lua(g)).collect();
        out += &format!("  ignore = {{ {} }},\n", globs.join(", "));
    }
    out + "}\n"
}

/// The modules `names` — every syncable one when empty — each with its
/// plan.
pub fn plan_modules<'r>(
    repo: &'r Repo,
    names: &[String],
    direction: Direction,
    crypto: &Crypto,
    state: &State,
    opts: PlanOpts,
) -> Result<Vec<(&'r Module, Plan)>> {
    let selected: Vec<&Module> = if names.is_empty() {
        repo.syncable().collect()
    } else {
        names
            .iter()
            .map(|n| {
                let m = repo.module(n)?;
                if m.dest.is_none() {
                    bail!("module `{n}` has no `path` and cannot be synced");
                }
                Ok(m)
            })
            .collect::<Result<_>>()?
    };
    selected
        .into_iter()
        .map(|m| Ok((m, plan(m, direction, crypto, state, opts)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder under the home is written as `qd.path.home(…)` by its
    /// parts; one elsewhere as it is; ignores as a list.
    #[test]
    fn a_module_file_names_its_path_from_the_home() {
        let home = [PathBuf::from("/h/me")];
        assert_eq!(
            module_file(
                Path::new("/h/me/.config/kawoosh"),
                &home,
                &["fonts/**".into()]
            ),
            "local qd = require(\"qd\")\n\nreturn {\n  path = qd.path.home(\".config\", \"kawoosh\"),\n  ignore = { \"fonts/**\" },\n}\n"
        );
        assert_eq!(
            module_file(Path::new("/etc/x"), &home, &[]),
            "local qd = require(\"qd\")\n\nreturn {\n  path = \"/etc/x\",\n}\n"
        );
    }
}
