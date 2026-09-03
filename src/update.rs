//! `qd self-update`: find the newest version in the Forgejo generic package
//! registry, verify the SHA-256, swap the running executable.
//!
//! Layout published by CI (see .forgejo/workflows/ci.yml):
//!   {host}/api/packages/{owner}/generic/{package}/{version}/qd-<os>-<arch>[.exe]
//!   {host}/api/packages/{owner}/generic/{package}/{version}/checksums.sha256

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const DEFAULT_HOST: &str = match option_env!("QD_UPDATE_HOST") {
    Some(v) => v,
    None => "https://drydock9.qxuken.dev",
};
pub const DEFAULT_OWNER: &str = match option_env!("QD_UPDATE_OWNER") {
    Some(v) => v,
    None => "qxuken",
};
pub const DEFAULT_PACKAGE: &str = "qd";
pub const CHECKSUMS_FILE: &str = "checksums.sha256";

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    version: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate(String),
    Updated { from: String, to: String },
    Available { from: String, to: String },
}

pub struct Source {
    pub host: String,
    pub owner: String,
    pub package: String,
    /// Optional token for private packages (`Authorization: token …`).
    pub token: Option<String>,
}

impl Source {
    fn versions_url(&self) -> String {
        format!(
            "{}/api/v1/packages/{}?type=generic&q={}&limit=100",
            self.host.trim_end_matches('/'),
            self.owner,
            self.package
        )
    }

    fn file_url(&self, version: &str, file: &str) -> String {
        format!(
            "{}/api/packages/{}/generic/{}/{version}/{file}",
            self.host.trim_end_matches('/'),
            self.owner,
            self.package
        )
    }

    fn get(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let mut req = ureq::get(url);
        if let Some(t) = &self.token {
            req = req.header("Authorization", &format!("token {t}"));
        }
        req.call().with_context(|| format!("fetching {url}"))
    }
}

/// Asset name for this build, e.g. `qd-macos-aarch64` or `qd-windows-x86_64.exe`.
pub fn asset_name() -> String {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    format!(
        "qd-{}-{}{ext}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// `(major, minor, patch, is_release)`; prereleases sort below the release.
fn version_key(v: &str) -> (u64, u64, u64, bool) {
    let v = v.trim_start_matches('v');
    let (main, pre) = v.split_once('-').unwrap_or((v, ""));
    let mut it = main.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        pre.is_empty(),
    )
}

pub fn latest_version(src: &Source) -> Result<String> {
    let url = src.versions_url();
    let packages: Vec<Package> = src
        .get(&url)?
        .body_mut()
        .read_json()
        .context("parsing package list")?;
    packages
        .into_iter()
        .filter(|p| p.name == src.package)
        .map(|p| p.version)
        .max_by_key(|v| version_key(v))
        .with_context(|| format!("no versions of package `{}` at {url}", src.package))
}

pub fn self_update(src: &Source, dry_run: bool) -> Result<Outcome> {
    let latest = latest_version(src)?;
    let current = current_version().to_owned();
    if version_key(&latest) <= version_key(&current) {
        return Ok(Outcome::UpToDate(current));
    }
    if dry_run {
        return Ok(Outcome::Available {
            from: current,
            to: latest,
        });
    }

    let name = asset_name();
    let sums = download(src, &src.file_url(&latest, CHECKSUMS_FILE))?;
    let expected = String::from_utf8_lossy(&sums)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?, it.next()?.trim_start_matches('*')))
        })
        .find(|(_, n)| *n == name)
        .map(|(h, _)| h.to_lowercase())
        .with_context(|| format!("{CHECKSUMS_FILE} for {latest} has no entry for {name}"))?;

    let bytes = download(src, &src.file_url(&latest, &name))?;
    let actual = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != expected {
        bail!("checksum mismatch for {name}: expected {expected}, got {actual}");
    }

    let exe = std::env::current_exe().context("locating the running executable")?;
    let tmp = exe.with_extension("update-tmp");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    self_replace::self_replace(&tmp).context("replacing the executable")?;
    let _ = std::fs::remove_file(&tmp);
    Ok(Outcome::Updated {
        from: current,
        to: latest,
    })
}

fn download(src: &Source, url: &str) -> Result<Vec<u8>> {
    src.get(url)?
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()
        .with_context(|| format!("reading {url}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order() {
        assert!(version_key("0.2.0") > version_key("0.1.9"));
        assert!(version_key("1.0.0") > version_key("1.0.0-alpha.3"));
        assert!(version_key("v0.3.1") > version_key("0.3.0"));
        assert_eq!(version_key("0.1.0"), version_key("v0.1.0"));
    }
}
