//! Config types.
//!
//! `Raw*` is exactly what a `qd.lua` file returns, deserialized through serde.
//! The resolved types in this file are what the rest of the program consumes:
//! absolute paths, compiled globs, root defaults already merged in.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::host::Host;
use crate::lua::Hooks;

// ---------------------------------------------------------------------------
// Raw (as returned by qd.lua)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawModule {
    pub enabled: Option<bool>,
    pub path: Option<String>,
    pub brew: Vec<Package>,
    pub scoop: Vec<Package>,
    pub files: Vec<RawFilePair>,
    pub include: Vec<String>,
    pub ignore: Vec<String>,
    pub encrypt: Vec<String>,
    pub dotfile: RawDotfile,
    pub setup: Option<RawSetup>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Package {
    Name(String),
    Detailed {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tap: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bucket: Option<String>,
    },
}

impl Package {
    pub fn name(&self) -> &str {
        match self {
            Package::Name(n) => n,
            Package::Detailed { name, .. } => name,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawFilePair {
    pub src: String,
    pub dest: String,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct RawDotfile {
    pub include: Vec<String>,
    pub source: Vec<String>,
    pub env_include: Vec<String>,
    pub env_source: Vec<String>,
}

/// `before` / `after` are functions and are pulled out by the Lua host before
/// serde sees the table, so only `version` remains here.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawSetup {
    pub version: Option<u32>,
}

// ---------------------------------------------------------------------------
// Resolved
// ---------------------------------------------------------------------------

/// Settings from the root `qd.lua` that apply to every module.
#[derive(Debug, Default, Clone, Serialize)]
pub struct RootConfig {
    pub ignore: Vec<String>,
    pub encrypt: Vec<String>,
    pub include: Vec<PathBuf>,
    pub dotfile: Dotfile,
}

impl RootConfig {
    pub fn from_raw(raw: RawModule, src: &Path) -> Result<RootConfig> {
        if raw.path.is_some() {
            bail!("root qd.lua must not set `path`");
        }
        if raw.setup.is_some()
            || !raw.files.is_empty()
            || !raw.brew.is_empty()
            || !raw.scoop.is_empty()
        {
            bail!("root qd.lua only supports `ignore`, `encrypt`, `include` and `dotfile`");
        }
        Ok(RootConfig {
            ignore: raw.ignore,
            encrypt: raw.encrypt,
            include: raw
                .include
                .iter()
                .map(|p| absolute(p, None, "include", src))
                .collect::<Result<_>>()?,
            dotfile: Dotfile::from_raw(&raw.dotfile, None, src)?,
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FilePair {
    pub src: PathBuf,
    pub dest: PathBuf,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct Dotfile {
    pub include: Vec<PathBuf>,
    pub source: Vec<PathBuf>,
    pub env_include: Vec<PathBuf>,
    pub env_source: Vec<PathBuf>,
}

impl Dotfile {
    fn from_raw(raw: &RawDotfile, dest: Option<&Path>, src: &Path) -> Result<Dotfile> {
        let conv = |items: &[String], field: &str| -> Result<Vec<PathBuf>> {
            items
                .iter()
                .map(|p| absolute(p, dest, field, src))
                .collect()
        };
        Ok(Dotfile {
            include: conv(&raw.include, "dotfile.include")?,
            source: conv(&raw.source, "dotfile.source")?,
            env_include: conv(&raw.env_include, "dotfile.env_include")?,
            env_source: conv(&raw.env_source, "dotfile.env_source")?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_empty()
            && self.source.is_empty()
            && self.env_include.is_empty()
            && self.env_source.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Setup {
    pub version: u32,
    pub before: bool,
    pub after: bool,
}

pub struct Module {
    pub name: String,
    pub src: PathBuf,
    pub dest: Option<PathBuf>,
    pub enabled: bool,
    /// Glob patterns (root + module), relative to `dest`.
    pub ignore: Vec<String>,
    pub encrypt: Vec<String>,
    pub ignore_set: GlobSet,
    pub encrypt_set: GlobSet,
    pub files: Vec<FilePair>,
    pub include: Vec<PathBuf>,
    pub brew: Vec<Package>,
    pub scoop: Vec<Package>,
    pub dotfile: Dotfile,
    pub setup: Option<Setup>,
    pub hooks: Hooks,
}

impl std::fmt::Debug for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Module")
            .field("name", &self.name)
            .field("dest", &self.dest)
            .finish_non_exhaustive()
    }
}

/// Serializable projection of `Module`, used by `qd show` and passed to hooks.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleView {
    pub name: String,
    pub src: PathBuf,
    pub dest: Option<PathBuf>,
    pub enabled: bool,
    pub ignore: Vec<String>,
    pub encrypt: Vec<String>,
    pub files: Vec<FilePair>,
    pub include: Vec<PathBuf>,
    pub brew: Vec<Package>,
    pub scoop: Vec<Package>,
    pub dotfile: Dotfile,
    pub setup: Option<Setup>,
}

impl Module {
    pub fn resolve(
        _host: &Host,
        root: &RootConfig,
        name: String,
        src: PathBuf,
        raw: RawModule,
        hooks: Hooks,
    ) -> Result<Module> {
        let ctx = |what: &str| format!("module `{name}`: {what}");

        let dest = match raw.path {
            Some(p) => {
                let p = PathBuf::from(p);
                if !p.is_absolute() {
                    bail!(ctx(&format!(
                        "`path` must be absolute, got `{}`",
                        p.display()
                    )));
                }
                Some(p)
            }
            None => None,
        };
        let dest_ref = dest.as_deref();

        let ignore: Vec<String> = root
            .ignore
            .iter()
            .chain(raw.ignore.iter())
            .cloned()
            .collect();
        let encrypt: Vec<String> = root
            .encrypt
            .iter()
            .chain(raw.encrypt.iter())
            .cloned()
            .collect();
        let ignore_set = compile_globs(&ignore).with_context(|| ctx("invalid `ignore` glob"))?;
        let encrypt_set = compile_globs(&encrypt).with_context(|| ctx("invalid `encrypt` glob"))?;

        let mut include: Vec<PathBuf> = root.include.clone();
        for p in &raw.include {
            include.push(
                absolute(p, None, "include", &src).with_context(|| ctx("bad `include` entry"))?,
            );
        }

        let mut files = Vec::new();
        for f in raw.files {
            if !f.enabled.unwrap_or(true) {
                continue;
            }
            files.push(FilePair {
                src: absolute(&f.src, Some(&src), "files[].src", &src)
                    .with_context(|| ctx("bad `files` entry"))?,
                dest: absolute(&f.dest, None, "files[].dest", &src)
                    .with_context(|| ctx("bad `files` entry"))?,
            });
        }

        let dotfile = Dotfile::from_raw(&raw.dotfile, dest_ref, &src)
            .with_context(|| ctx("bad `dotfile` entry"))?;

        let setup = match raw.setup {
            Some(s) => Some(Setup {
                version: s.version.unwrap_or(1),
                before: hooks.has_before(),
                after: hooks.has_after(),
            }),
            None if hooks.has_before() || hooks.has_after() => Some(Setup {
                version: 1,
                before: hooks.has_before(),
                after: hooks.has_after(),
            }),
            None => None,
        };

        Ok(Module {
            enabled: raw.enabled.unwrap_or(true),
            name,
            src,
            dest,
            ignore,
            encrypt,
            ignore_set,
            encrypt_set,
            files,
            include,
            brew: raw.brew,
            scoop: raw.scoop,
            dotfile,
            setup,
            hooks,
        })
    }

    pub fn view(&self) -> ModuleView {
        ModuleView {
            name: self.name.clone(),
            src: self.src.clone(),
            dest: self.dest.clone(),
            enabled: self.enabled,
            ignore: self.ignore.clone(),
            encrypt: self.encrypt.clone(),
            files: self.files.clone(),
            include: self.include.clone(),
            brew: self.brew.clone(),
            scoop: self.scoop.clone(),
            dotfile: self.dotfile.clone(),
            setup: self.setup.clone(),
        }
    }
}

/// Resolve a config path: absolute stays, relative is joined onto `base`.
/// Without a base a relative path is an error naming the field.
fn absolute(p: &str, base: Option<&Path>, field: &str, src: &Path) -> Result<PathBuf> {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        return Ok(path);
    }
    match base {
        Some(b) => Ok(b.join(path)),
        None => bail!(
            "`{field}` entry `{p}` is relative but the module has no `path` (in {})",
            src.join("qd.lua").display()
        ),
    }
}

/// Globs use nushell semantics: `*` does not cross directory separators, `**` does.
pub fn compile_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = GlobBuilder::new(p)
            .literal_separator(true)
            .build()
            .with_context(|| format!("glob `{p}`"))?;
        b.add(g);
    }
    Ok(b.build()?)
}
