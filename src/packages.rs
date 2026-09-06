//! Package actions. Every plugin with a `packages` section is a manager; the
//! plugin folds the module lists into commands and the core runs them.

use std::process::Command;

use anyhow::{Context, Result, bail};

pub use crate::plugin::Action;
use crate::plugin::Plugin;
use crate::repo::Repo;

/// Plugins that manage packages, in declaration order.
pub fn managers(repo: &Repo) -> Vec<&Plugin> {
    repo.plugins
        .iter()
        .filter(|p| p.manages_packages())
        .collect()
}

/// First manager whose `available()` holds on this host.
pub fn detect(repo: &Repo) -> Result<Option<&Plugin>> {
    for p in managers(repo) {
        if p.available()? {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// The manager named by `--manager`, or the detected one.
pub fn pick<'r>(repo: &'r Repo, name: Option<&str>) -> Result<&'r Plugin> {
    match name {
        Some(n) => {
            let p = repo
                .plugins
                .get(n)
                .with_context(|| format!("no plugin named `{n}`"))?;
            if !p.manages_packages() {
                bail!("plugin `{n}` has no `packages` section");
            }
            Ok(p)
        }
        None => detect(repo)?.with_context(|| {
            let names: Vec<&str> = managers(repo).iter().map(|p| p.name()).collect();
            format!(
                "no package manager found on PATH (checked {}); pass --manager",
                names.join(", ")
            )
        }),
    }
}

/// Run the plugin's commands for `action`, printing each first.
pub fn run(repo: &Repo, plugin: &Plugin, action: Action, dry_run: bool) -> Result<()> {
    let commands = plugin.package_commands(action, &repo.ctx())?;
    if commands.is_empty() {
        println!("no {} packages configured", plugin.name());
        return Ok(());
    }
    for argv in &commands {
        exec(argv, dry_run)?;
    }
    Ok(())
}

pub fn list(repo: &Repo, plugin: &Plugin) -> Result<serde_json::Value> {
    plugin.package_list(&repo.ctx())
}

fn exec(argv: &[String], dry_run: bool) -> Result<()> {
    println!("$ {}", argv.join(" "));
    if dry_run {
        return Ok(());
    }
    let (program, args) = argv.split_first().context("empty command")?;
    let status = command(program)
        .args(args)
        .status()
        .with_context(|| format!("cannot start {program}"))?;
    if !status.success() {
        bail!("{} exited with {status}", argv.join(" "));
    }
    Ok(())
}

/// Package managers on Windows are batch or PowerShell shims, so go through
/// `cmd /C` there.
fn command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", program]);
        c
    } else {
        Command::new(program)
    }
}
