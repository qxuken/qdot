//! `qd self-update`: read the published version, verify the SHA-256, swap the
//! running executable.
//!
//! Discovery is three plain GETs against a base URL, not a forge's API:
//!
//!   {base}/version.txt        the published version, one line
//!   {base}/checksums.sha256   a SHA-256 per asset
//!   {base}/qd-<os>-<arch>     the binary for this platform
//!
//! Nothing here knows what serves that base. GitHub does, natively, at
//! `.../releases/latest/download/`; so can S3, R2, a Worker, nginx or a
//! Forgejo generic registry. That is the point. The Forgejo → GitHub move
//! stranded every installed binary, because the endpoint they were built
//! against could only speak Forgejo's API — no flag could redirect them, and
//! each machine had to be reinstalled by hand. A base URL that keeps serving
//! three files is enough to roll a fleet onto a new home instead: point the
//! old base at the new one for a release, and every machine updates itself.
//!
//! `--url` with `--sha256` skips discovery and installs one named binary, for
//! the case where even the base URL has gone.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// Where the current release's files are served. Baked in at build time so a
/// fork or a self-hosted mirror needs no flags.
pub const DEFAULT_BASE: &str = match option_env!("QD_UPDATE_BASE_URL") {
    Some(v) => v,
    None => "https://github.com/qxuken/qdot/releases/latest/download",
};
pub const CHECKSUMS_FILE: &str = "checksums.sha256";
pub const VERSION_FILE: &str = "version.txt";

const UA: &str = concat!("qd/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate(String),
    Updated { from: String, to: String },
    Available { from: String, to: String },
    Replaced { from: String, url: String },
}

pub struct Source {
    pub base: String,
    /// For a base URL behind auth. ureq defaults to
    /// `RedirectAuthHeaders::Never`, so it is dropped on a redirect and never
    /// reaches a CDN that would reject a signed URL carrying one.
    pub token: Option<String>,
}

impl Source {
    fn url(&self, file: &str) -> String {
        format!("{}/{file}", self.base.trim_end_matches('/'))
    }

    fn get(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let mut req = ureq::get(url).header("User-Agent", UA);
        if let Some(t) = &self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        req.call().with_context(|| format!("fetching {url}"))
    }

    fn text(&self, file: &str) -> Result<String> {
        let url = self.url(file);
        self.get(&url)?
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading {url}"))
    }

    fn bytes(&self, file: &str) -> Result<Vec<u8>> {
        let url = self.url(file);
        self.get(&url)?
            .body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_to_vec()
            .with_context(|| format!("reading {url}"))
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

/// `version.txt` as published: one line, with or without a leading `v`.
fn parse_version(body: &str) -> Result<String> {
    let v = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches('v').to_owned())
        .filter(|v| !v.is_empty())
        .context("no version in it")?;
    if !v.starts_with(|c: char| c.is_ascii_digit()) {
        bail!("`{v}` does not look like a version");
    }
    Ok(v)
}

/// The SHA-256 recorded for `name`, from a `sha256sum`-style file.
fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            // `sha256sum` writes `*name` for a binary-mode entry.
            Some((it.next()?, it.next()?.trim_start_matches('*')))
        })
        .find(|(_, n)| *n == name)
        .map(|(h, _)| h.to_lowercase())
}

fn expect_sha256(h: &str) -> Result<String> {
    let h = h.trim().to_lowercase();
    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("`{h}` is not a SHA-256 digest (64 hex characters)");
    }
    Ok(h)
}

pub fn latest_version(src: &Source) -> Result<String> {
    let body = src.text(VERSION_FILE)?;
    parse_version(&body).with_context(|| format!("{}: {VERSION_FILE}", src.base))
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
    let sums = src.text(CHECKSUMS_FILE)?;
    let expected = checksum_for(&sums, &name)
        .with_context(|| format!("{CHECKSUMS_FILE} for {latest} has no entry for {name}"))?;
    let bytes = src.bytes(&name)?;
    verify(&bytes, &expected, &name)?;
    swap(&bytes)?;
    Ok(Outcome::Updated {
        from: current,
        to: latest,
    })
}

/// Install exactly this binary, checked against `sha256`. No discovery, so it
/// works against any host — including one this build has never heard of.
pub fn update_from_url(url: &str, sha256: &str) -> Result<Outcome> {
    let expected = expect_sha256(sha256)?;
    let src = Source {
        base: String::new(),
        token: None,
    };
    let bytes = src
        .get(url)?
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()
        .with_context(|| format!("reading {url}"))?;
    verify(&bytes, &expected, url)?;
    swap(&bytes)?;
    Ok(Outcome::Replaced {
        from: current_version().to_owned(),
        url: url.to_owned(),
    })
}

fn verify(bytes: &[u8], expected: &str, what: &str) -> Result<()> {
    let actual = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != expected {
        bail!("checksum mismatch for {what}: expected {expected}, got {actual}");
    }
    Ok(())
}

fn swap(bytes: &[u8]) -> Result<()> {
    let exe = std::env::current_exe().context("locating the running executable")?;
    let tmp = exe.with_extension("update-tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    self_replace::self_replace(&tmp).context("replacing the executable")?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(base: &str) -> Source {
        Source {
            base: base.into(),
            token: None,
        }
    }

    #[test]
    fn versions_order() {
        assert!(version_key("0.2.0") > version_key("0.1.9"));
        assert!(version_key("1.0.0") > version_key("1.0.0-alpha.3"));
        assert!(version_key("v0.3.1") > version_key("0.3.0"));
        assert_eq!(version_key("0.1.0"), version_key("v0.1.0"));
    }

    #[test]
    fn urls_join_with_one_slash() {
        assert_eq!(
            src("https://h/d").url("version.txt"),
            "https://h/d/version.txt"
        );
        assert_eq!(
            src("https://h/d/").url("version.txt"),
            "https://h/d/version.txt"
        );
    }

    #[test]
    fn version_file_is_one_line_either_spelling() {
        assert_eq!(parse_version("0.4.0\n").unwrap(), "0.4.0");
        assert_eq!(parse_version("v0.4.0").unwrap(), "0.4.0");
        assert_eq!(parse_version("\n  v1.2.3-rc.1  \n").unwrap(), "1.2.3-rc.1");
        // A base URL that answers every path with a page, rather than 404,
        // would otherwise have us compare against a chunk of HTML.
        assert!(parse_version("<!doctype html>").is_err());
        assert!(parse_version("").is_err());
    }

    #[test]
    fn checksums_are_read_by_asset_name() {
        let sums = concat!(
            "aa11  qd-linux-x86_64\n",
            "BB22 *qd-macos-aarch64\n",
            "cc33  checksums-of-something-else\n"
        );
        assert_eq!(
            checksum_for(sums, "qd-linux-x86_64").as_deref(),
            Some("aa11")
        );
        // Binary-mode `*` stripped, hex lowercased.
        assert_eq!(
            checksum_for(sums, "qd-macos-aarch64").as_deref(),
            Some("bb22")
        );
        assert_eq!(checksum_for(sums, "qd-windows-x86_64.exe"), None);
    }

    #[test]
    fn a_digest_has_to_look_like_one() {
        let good = "a".repeat(64);
        assert_eq!(expect_sha256(&good).unwrap(), good);
        assert_eq!(expect_sha256(&good.to_uppercase()).unwrap(), good);
        assert!(expect_sha256("deadbeef").is_err(), "too short");
        assert!(expect_sha256(&"z".repeat(64)).is_err(), "not hex");
    }
}
