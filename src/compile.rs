//! Generate the two Nushell entry files that `config.nu` / `env.nu` source.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::Dotfile;
use crate::repo::Repo;

pub const MAIN_FILE: &str = ".dotfiles.local.nu";
pub const ENV_FILE: &str = ".dotfiles-env.local.nu";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Compiled {
    pub main: Vec<String>,
    pub env: Vec<String>,
}

pub fn compile(repo: &Repo) -> Compiled {
    let mut out = Compiled::default();
    let mut add = |d: &Dotfile| {
        out.main.extend(d.include.iter().map(|p| line("use", p)));
        out.main.extend(d.source.iter().map(|p| line("source", p)));
        out.env.extend(d.env_include.iter().map(|p| line("use", p)));
        out.env
            .extend(d.env_source.iter().map(|p| line("source", p)));
    };
    for m in &repo.modules {
        add(&m.dotfile);
    }
    add(&repo.global.dotfile);
    // Same ordering as the Nushell tool: unique, reverse sorted, so `use`
    // lines come before `source` lines.
    for v in [&mut out.main, &mut out.env] {
        v.sort();
        v.dedup();
        v.reverse();
    }
    out
}

fn line(verb: &str, path: &Path) -> String {
    format!("{verb} `{}`", path.display())
}

pub fn write(repo: &Repo, home: &Path) -> Result<(PathBuf, PathBuf)> {
    let c = compile(repo);
    let main = home.join(MAIN_FILE);
    let env = home.join(ENV_FILE);
    write_lines(&main, &c.main)?;
    write_lines(&env, &c.env)?;
    Ok((main, env))
}

fn write_lines(path: &Path, lines: &[String]) -> Result<()> {
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}
