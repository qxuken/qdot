//! `qd self-update`: find the newest release on GitHub, verify the SHA-256,
//! swap the running executable.
//!
//! Layout published by CI (see .github/workflows/release.yml): the release
//! tagged `vX.Y.Z` carries one asset per platform, `qd-<os>-<arch>[.exe]`,
//! plus a `checksums.sha256` over all of them.
//!
//! The releases list comes from the API, which honours `--token` for a private
//! repository. The assets come from their public download URLs, which redirect
//! to a signed CDN URL that rejects an Authorization header — so a private
//! repository lists but does not download.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const DEFAULT_API: &str = match option_env!("QD_UPDATE_API") {
    Some(v) => v,
    None => "https://api.github.com",
};
pub const DEFAULT_OWNER: &str = match option_env!("QD_UPDATE_OWNER") {
    Some(v) => v,
    None => "qxuken",
};
pub const DEFAULT_REPO: &str = match option_env!("QD_UPDATE_REPO") {
    Some(v) => v,
    None => "qdot",
};
pub const CHECKSUMS_FILE: &str = "checksums.sha256";

/// GitHub rejects an API request without one.
const UA: &str = concat!("qd/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Release {
    /// The tag without its `v`, which is what `--version` prints.
    pub fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }

    fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate(String),
    Updated { from: String, to: String },
    Available { from: String, to: String },
}

pub struct Source {
    /// API base, `https://api.github.com` or a GitHub Enterprise one.
    pub api: String,
    pub owner: String,
    pub repo: String,
    /// Optional token for a private repository (`Authorization: Bearer …`).
    pub token: Option<String>,
}

impl Source {
    fn releases_url(&self) -> String {
        format!(
            "{}/repos/{}/{}/releases?per_page=100",
            self.api.trim_end_matches('/'),
            self.owner,
            self.repo
        )
    }

    fn api_get(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let mut req = ureq::get(url)
            .header("User-Agent", UA)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        req.call().with_context(|| format!("fetching {url}"))
    }

    /// The download URL of one of a release's assets.
    fn pick<'a>(&self, release: &'a Release, name: &str) -> Result<&'a str> {
        release
            .asset(name)
            .map(|a| a.browser_download_url.as_str())
            .with_context(|| format!("release {} has no asset {name}", release.tag_name))
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

/// The highest-versioned published release. Drafts are skipped; prereleases
/// are kept but rank below the release of the same version, so one only wins
/// when nothing newer is out.
fn pick_latest(releases: Vec<Release>) -> Option<Release> {
    releases
        .into_iter()
        .filter(|r| !r.draft)
        .max_by_key(|r| version_key(&r.tag_name))
}

pub fn latest_release(src: &Source) -> Result<Release> {
    let url = src.releases_url();
    let releases: Vec<Release> = src
        .api_get(&url)?
        .body_mut()
        .read_json()
        .context("parsing the releases list")?;
    pick_latest(releases).with_context(|| format!("no releases at {url}"))
}

pub fn latest_version(src: &Source) -> Result<String> {
    Ok(latest_release(src)?.version().to_owned())
}

pub fn self_update(src: &Source, dry_run: bool) -> Result<Outcome> {
    let release = latest_release(src)?;
    let latest = release.version().to_owned();
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
    let sums = download(src.pick(&release, CHECKSUMS_FILE)?)?;
    let expected = String::from_utf8_lossy(&sums)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?, it.next()?.trim_start_matches('*')))
        })
        .find(|(_, n)| *n == name)
        .map(|(h, _)| h.to_lowercase())
        .with_context(|| format!("{CHECKSUMS_FILE} for {latest} has no entry for {name}"))?;

    let bytes = download(src.pick(&release, &name)?)?;
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

/// Assets are public objects behind a redirect to a signed CDN URL; sending
/// the token along would make that URL reject the request.
fn download(url: &str) -> Result<Vec<u8>> {
    ureq::get(url)
        .header("User-Agent", UA)
        .call()
        .with_context(|| format!("fetching {url}"))?
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

    fn releases(json: &str) -> Vec<Release> {
        serde_json::from_str(json).expect("fixture parses")
    }

    #[test]
    fn newest_release_wins_and_drafts_lose() {
        let rs = releases(
            r#"[
              {"tag_name": "v0.2.0", "assets": []},
              {"tag_name": "v0.4.0", "draft": true, "assets": []},
              {"tag_name": "v0.3.0", "assets": []}
            ]"#,
        );
        let latest = pick_latest(rs).expect("a release");
        assert_eq!(latest.tag_name, "v0.3.0");
        assert_eq!(latest.version(), "0.3.0");
    }

    #[test]
    fn assets_are_found_by_name() {
        let rs = releases(
            r#"[{"tag_name": "v1.0.0", "assets": [
              {"name": "qd-linux-x86_64", "browser_download_url": "https://example.invalid/qd"},
              {"name": "checksums.sha256", "browser_download_url": "https://example.invalid/sums"}
            ]}]"#,
        );
        let r = pick_latest(rs).expect("a release");
        let src = Source {
            api: DEFAULT_API.into(),
            owner: "o".into(),
            repo: "r".into(),
            token: None,
        };
        assert_eq!(
            src.pick(&r, CHECKSUMS_FILE).unwrap(),
            "https://example.invalid/sums"
        );
        assert!(src.pick(&r, "qd-macos-aarch64").is_err());
    }

    #[test]
    fn no_releases_is_an_error() {
        assert!(pick_latest(releases("[]")).is_none());
    }
}
