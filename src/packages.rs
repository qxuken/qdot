//! Fold every module's package lists and drive brew or scoop.

use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::Package;
use crate::repo::Repo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Manager {
    Brew,
    Scoop,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Brew => "brew",
            Manager::Scoop => "scoop",
        }
    }

    /// First manager found on PATH, brew before scoop.
    pub fn detect() -> Option<Manager> {
        [Manager::Brew, Manager::Scoop]
            .into_iter()
            .find(|m| m.available())
    }

    pub fn available(self) -> bool {
        command(self)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PackageSet {
    pub packages: Vec<String>,
    /// brew taps or scoop buckets, in first-seen order.
    pub sources: Vec<String>,
}

pub fn collect(repo: &Repo, manager: Manager) -> PackageSet {
    let mut set = PackageSet::default();
    let mut seen = std::collections::HashSet::new();
    for m in &repo.modules {
        let list = match manager {
            Manager::Brew => &m.brew,
            Manager::Scoop => &m.scoop,
        };
        for p in list {
            if !seen.insert(p.name().to_owned()) {
                continue;
            }
            match (manager, p) {
                (
                    Manager::Brew,
                    Package::Detailed {
                        name,
                        tap: Some(tap),
                        ..
                    },
                ) => {
                    push_unique(&mut set.sources, tap);
                    set.packages.push(name.clone());
                }
                (
                    Manager::Scoop,
                    Package::Detailed {
                        name,
                        bucket: Some(bucket),
                        ..
                    },
                ) => {
                    push_unique(&mut set.sources, bucket);
                    set.packages.push(format!("{bucket}/{name}"));
                }
                (Manager::Scoop, p) => {
                    push_unique(&mut set.sources, "main");
                    set.packages.push(format!("main/{}", p.name()));
                }
                (Manager::Brew, p) => set.packages.push(p.name().to_owned()),
            }
        }
    }
    set
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|x| x == s) {
        v.push(s.to_owned());
    }
}

pub fn install(manager: Manager, set: &PackageSet, dry_run: bool) -> Result<()> {
    if set.packages.is_empty() {
        println!("no {} packages configured", manager.name());
        return Ok(());
    }
    for src in &set.sources {
        match manager {
            Manager::Brew => run(manager, &["tap", src], dry_run)?,
            Manager::Scoop => run(manager, &["bucket", "add", src], dry_run)?,
        }
    }
    let mut args = vec!["install"];
    args.extend(set.packages.iter().map(String::as_str));
    run(manager, &args, dry_run)
}

pub fn upgrade(manager: Manager, set: &PackageSet, dry_run: bool) -> Result<()> {
    if set.packages.is_empty() {
        println!("no {} packages configured", manager.name());
        return Ok(());
    }
    let verb = match manager {
        Manager::Brew => "upgrade",
        Manager::Scoop => "update",
    };
    let mut args = vec![verb];
    args.extend(set.packages.iter().map(String::as_str));
    run(manager, &args, dry_run)
}

fn command(manager: Manager) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", manager.name()]);
        c
    } else {
        Command::new(manager.name())
    }
}

fn run(manager: Manager, args: &[&str], dry_run: bool) -> Result<()> {
    println!("$ {} {}", manager.name(), args.join(" "));
    if dry_run {
        return Ok(());
    }
    let status = command(manager)
        .args(args)
        .status()
        .with_context(|| format!("cannot start {}", manager.name()))?;
    if !status.success() {
        bail!(
            "{} {} exited with {status}",
            manager.name(),
            args.first().unwrap_or(&"")
        );
    }
    Ok(())
}
