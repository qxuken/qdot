//! Walk one side of a module into a map of relative path → file entry.
//!
//! Keys are `/`-separated relative paths without the `.age` suffix, so the
//! repo side and the destination side can be compared directly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use globset::GlobSet;
use walkdir::WalkDir;

use crate::discover::CONFIG_FILE;

pub const AGE_EXT: &str = "age";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Actual file on disk (with `.age` on the repo side when encrypted).
    pub abs: PathBuf,
    /// The bytes on disk are age ciphertext.
    pub encrypted: bool,
}

pub type Tree = BTreeMap<String, Entry>;

/// Repo side: every file except the module's `qd.lua`; `*.age` marks encrypted.
pub fn scan_src(dir: &Path, ignore: &GlobSet) -> Result<Tree> {
    let mut tree = Tree::new();
    if !dir.is_dir() {
        return Ok(tree);
    }
    for (rel, abs) in walk(dir)? {
        if rel == CONFIG_FILE || ignore.is_match(&rel) {
            continue;
        }
        match rel.strip_suffix(&format!(".{AGE_EXT}")) {
            Some(plain) => {
                tree.insert(
                    plain.to_owned(),
                    Entry {
                        abs,
                        encrypted: true,
                    },
                );
            }
            None => {
                tree.insert(
                    rel,
                    Entry {
                        abs,
                        encrypted: false,
                    },
                );
            }
        }
    }
    Ok(tree)
}

/// Destination side: skip ignored files and stray `*.age`. Everything here is
/// plaintext; whether it is stored encrypted in the repo is the planner's call.
pub fn scan_dest(dir: &Path, ignore: &GlobSet) -> Result<Tree> {
    let mut tree = Tree::new();
    if !dir.is_dir() {
        return Ok(tree);
    }
    for (rel, abs) in walk(dir)? {
        if ignore.is_match(&rel) || rel.ends_with(&format!(".{AGE_EXT}")) {
            continue;
        }
        tree.insert(
            rel,
            Entry {
                abs,
                encrypted: false,
            },
        );
    }
    Ok(tree)
}

fn walk(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(dir).follow_links(true).min_depth(1) {
        let entry = entry.with_context(|| format!("walking {}", dir.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(dir)?;
        out.push((to_slash(rel), entry.path().to_path_buf()));
    }
    Ok(out)
}

pub fn to_slash(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::compile_globs;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn src_strips_age_and_skips_config() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        touch(&d.join("qd.lua"));
        touch(&d.join("a.txt"));
        touch(&d.join("sub/secret.p12.age"));
        touch(&d.join(".DS_Store"));
        let ignore = compile_globs(&["**/.DS_Store".into()]).unwrap();
        let t = scan_src(d, &ignore).unwrap();
        let keys: Vec<_> = t.keys().collect();
        assert_eq!(keys, ["a.txt", "sub/secret.p12"]);
        assert!(t["sub/secret.p12"].encrypted);
        assert_eq!(t["sub/secret.p12"].abs, d.join("sub/secret.p12.age"));
    }

    #[test]
    fn dest_applies_ignore_globs_and_skips_age() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        touch(&d.join("config.yml"));
        touch(&d.join("history.txt"));
        touch(&d.join("stray.age"));
        touch(&d.join("deep/x.p12"));
        let ignore = compile_globs(&["**/history.txt".into()]).unwrap();
        let t = scan_dest(d, &ignore).unwrap();
        let keys: Vec<_> = t.keys().collect();
        assert_eq!(keys, ["config.yml", "deep/x.p12"]);
        assert!(t.values().all(|e| !e.encrypted));
    }

    #[test]
    fn missing_dir_is_empty() {
        let ignore = compile_globs(&[]).unwrap();
        assert!(
            scan_src(Path::new("/definitely/not/here"), &ignore)
                .unwrap()
                .is_empty()
        );
    }
}
