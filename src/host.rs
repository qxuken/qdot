//! Everything the config layer is allowed to know about the machine.
//!
//! `Host` is built once from the environment (or directly from paths in tests)
//! and every later stage reads from it instead of touching `std::env` again.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Darwin,
    Linux,
    Windows,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Darwin
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Linux
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Os::Darwin => "darwin",
            Os::Linux => "linux",
            Os::Windows => "windows",
        }
    }

    pub fn is_posix(self) -> bool {
        !matches!(self, Os::Windows)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Host {
    pub os: Os,
    /// `ID` from `/etc/os-release` on Linux, e.g. `ubuntu`.
    pub distro: Option<String>,
    pub wsl: bool,
    pub tags: BTreeSet<String>,
    pub home: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    pub app_support: PathBuf,
    pub appdata: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    /// Root of the dotfiles repository.
    pub dotfiles: PathBuf,
}

impl Host {
    /// Base constructor: derives the standard directories from `home` and reads
    /// nothing from the environment. Tags and Windows dirs start empty.
    pub fn new(os: Os, home: impl Into<PathBuf>, dotfiles: impl Into<PathBuf>) -> Host {
        let home = home.into();
        Host {
            os,
            distro: None,
            wsl: false,
            tags: BTreeSet::new(),
            config: home.join(".config"),
            cache: home.join(".dotfiles-cache"),
            app_support: home.join("Library").join("Application Support"),
            appdata: None,
            local_appdata: None,
            home,
            dotfiles: dotfiles.into(),
        }
    }

    /// Build from the real environment. `tags` are the machine tags from state;
    /// `DOTFILES_TAGS` (comma or space separated) and the WSL auto tag are added.
    pub fn detect(
        dotfiles: impl Into<PathBuf>,
        tags: impl IntoIterator<Item = String>,
    ) -> Result<Host> {
        let home = dirs::home_dir().context("cannot determine the home directory")?;
        let mut host = Host::new(Os::current(), home, dotfiles);
        host.tags = tags.into_iter().collect();
        if let Ok(v) = std::env::var("DOTFILES_TAGS") {
            host.tags.extend(
                v.split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            );
        }
        host.wsl = std::env::var_os("WSL_DISTRO_NAME").is_some();
        if host.wsl {
            host.tags.insert("wsl".to_owned());
        }
        if host.os == Os::Linux {
            host.distro = read_os_release_id(Path::new("/etc/os-release"));
        }
        host.appdata = std::env::var_os("APPDATA").map(PathBuf::from);
        host.local_appdata = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        Ok(host)
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.contains(tag)
    }
}

fn read_os_release_id(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("ID="))
        .map(|v| v.trim().trim_matches('"').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_dirs_follow_home() {
        let h = Host::new(Os::Darwin, "/h", "/h/dotfiles");
        assert_eq!(h.config, PathBuf::from("/h/.config"));
        assert_eq!(h.cache, PathBuf::from("/h/.dotfiles-cache"));
        assert_eq!(
            h.app_support,
            PathBuf::from("/h/Library/Application Support")
        );
        assert!(h.tags.is_empty());
    }
}
