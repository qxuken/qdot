//! Version-control boundary. The first backend shells out to system git;
//! a libgit2 backend can implement the same trait later.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub trait Vcs {
    fn clone(&self, url: &str, dest: &Path) -> Result<()>;
    fn pull(&self, repo: &Path) -> Result<()>;
    fn commit_push(&self, repo: &Path, message: &str) -> Result<bool>;
    fn diff(&self, repo: &Path) -> Result<String>;
    fn is_dirty(&self, repo: &Path) -> Result<bool>;
}

pub struct SystemGit;

impl SystemGit {
    fn git(repo: Option<&Path>, args: &[&str]) -> Command {
        let mut c = Command::new("git");
        if let Some(r) = repo {
            c.arg("-C").arg(r);
        }
        c.args(args);
        c
    }

    fn run(repo: Option<&Path>, args: &[&str]) -> Result<()> {
        let status = Self::git(repo, args).status().context("cannot start git")?;
        if !status.success() {
            bail!("git {} exited with {status}", args.join(" "));
        }
        Ok(())
    }

    fn capture(repo: Option<&Path>, args: &[&str]) -> Result<String> {
        let out = Self::git(repo, args)
            .stderr(Stdio::inherit())
            .output()
            .context("cannot start git")?;
        if !out.status.success() {
            bail!("git {} exited with {}", args.join(" "), out.status);
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl Vcs for SystemGit {
    fn clone(&self, url: &str, dest: &Path) -> Result<()> {
        Self::run(None, &["clone", url, &dest.to_string_lossy()])
    }

    fn pull(&self, repo: &Path) -> Result<()> {
        Self::run(Some(repo), &["pull", "--ff-only"])
    }

    /// Stage everything, commit, push. Returns false when there was nothing to commit.
    fn commit_push(&self, repo: &Path, message: &str) -> Result<bool> {
        Self::run(Some(repo), &["add", "-A"])?;
        if !self.is_dirty(repo)? {
            return Ok(false);
        }
        Self::run(Some(repo), &["commit", "-m", message])?;
        Self::run(Some(repo), &["push"])?;
        Ok(true)
    }

    fn diff(&self, repo: &Path) -> Result<String> {
        let status = Self::capture(Some(repo), &["status", "--short"])?;
        let diff = Self::capture(Some(repo), &["diff"])?;
        Ok(format!(
            "{status}{}{diff}",
            if status.is_empty() || diff.is_empty() {
                ""
            } else {
                "\n"
            }
        ))
    }

    fn is_dirty(&self, repo: &Path) -> Result<bool> {
        Ok(!Self::capture(Some(repo), &["status", "--porcelain"])?
            .trim()
            .is_empty())
    }
}
