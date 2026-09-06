//! Plugins: Lua tables that extend the pipeline without touching the core.
//!
//! A plugin owns one top-level key in every `qd.lua` (its `name`) and may
//! provide, all optional and all pure:
//!
//! - `resolve(m, value)` — validate and normalise that key at load time
//!   (`m` is `{ name, src, dest, root }`); the result is what `qd show` prints
//!   and what the other functions receive.
//! - `compile(ctx)` — return `{ { path = ..., content = ... }, ... }`; the core
//!   diffs, writes atomically and journals them.
//! - `packages.install(ctx)` / `packages.upgrade(ctx)` — return a list of argv
//!   lists for the core to run; `packages.list(ctx)` returns anything
//!   printable; `available()` says whether this manager exists on the host.
//!
//! `ctx` is `{ root, global, modules }` with every enabled module as its
//! `qd show` view. The root `qd.lua` declares `plugins = { ... }`; when it
//! does not, the built-ins (`qd.nushell`, `qd.brew`, `qd.scoop`) are loaded.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use mlua::{Function, Lua, Table, Value};
use serde::{Deserialize, Serialize};

use crate::config::{ModuleView, RootConfig};
use crate::host::Host;
use crate::lua::{BUILTIN_PLUGINS, clean_error, from_lua, new_vm, to_lua};

/// Keys the core owns; a plugin cannot be named after one.
pub const CORE_FIELDS: &[&str] = &[
    "enabled", "path", "files", "include", "ignore", "encrypt", "setup", "plugins",
];

const PLUGIN_KEYS: &[&str] = &["name", "available", "resolve", "compile", "packages"];
const PACKAGE_KEYS: &[&str] = &["list", "install", "upgrade"];

/// Free-form plugin data as it appears under a module's plugin key.
pub type Ext = BTreeMap<String, serde_json::Value>;

/// What plugins see: the repo root, the root config and every enabled module.
#[derive(Debug, Serialize)]
pub struct Ctx<'a> {
    pub root: &'a Path,
    pub global: &'a RootConfig,
    pub modules: Vec<ModuleView>,
}

/// The file a plugin key belongs to, handed to `resolve`.
#[derive(Debug, Serialize)]
pub struct Target<'a> {
    pub name: &'a str,
    pub src: &'a Path,
    pub dest: Option<&'a Path>,
    /// True for the root `qd.lua`.
    pub root: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Upgrade,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Install => "install",
            Action::Upgrade => "upgrade",
        }
    }
}

/// One file a plugin wants on disk.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Output {
    pub plugin: String,
    pub path: PathBuf,
    pub content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutput {
    path: String,
    content: String,
}

pub struct Plugin {
    name: String,
    lua: Lua,
    available: Option<Function>,
    resolve: Option<Function>,
    compile: Option<Function>,
    packages: Option<PackageFns>,
}

struct PackageFns {
    list: Option<Function>,
    install: Option<Function>,
    upgrade: Option<Function>,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("name", &self.name)
            .field("compile", &self.compile.is_some())
            .field("packages", &self.packages.is_some())
            .finish()
    }
}

impl Plugin {
    fn from_table(lua: &Lua, t: Table) -> Result<Plugin> {
        let name = match t.get::<Value>("name")? {
            Value::String(s) => s.to_str()?.to_owned(),
            other => bail!("plugin without a string `name` (got {})", other.type_name()),
        };
        if name.is_empty() {
            bail!("plugin `name` is empty");
        }
        if CORE_FIELDS.contains(&name.as_str()) {
            bail!("plugin name `{name}` collides with a core field");
        }
        check_keys(&t, PLUGIN_KEYS).with_context(|| format!("plugin `{name}`"))?;

        let packages = match t.get::<Value>("packages")? {
            Value::Nil => None,
            Value::Table(p) => {
                check_keys(&p, PACKAGE_KEYS)
                    .with_context(|| format!("plugin `{name}`: `packages`"))?;
                Some(PackageFns {
                    list: func(&p, "list")?,
                    install: func(&p, "install")?,
                    upgrade: func(&p, "upgrade")?,
                })
            }
            other => bail!(
                "plugin `{name}`: `packages` must be a table, got {}",
                other.type_name()
            ),
        };

        Ok(Plugin {
            available: func(&t, "available").with_context(|| format!("plugin `{name}`"))?,
            resolve: func(&t, "resolve").with_context(|| format!("plugin `{name}`"))?,
            compile: func(&t, "compile").with_context(|| format!("plugin `{name}`"))?,
            packages,
            name,
            lua: lua.clone(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn compiles(&self) -> bool {
        self.compile.is_some()
    }

    pub fn manages_packages(&self) -> bool {
        self.packages.is_some()
    }

    /// Whether this plugin's package manager exists on the host. Plugins
    /// without `available` are always available.
    pub fn available(&self) -> Result<bool> {
        match &self.available {
            None => Ok(true),
            Some(f) => f
                .call::<bool>(())
                .map_err(clean_error)
                .with_context(|| format!("plugin `{}`: `available` failed", self.name)),
        }
    }

    fn resolve(&self, target: &Target, value: serde_json::Value) -> Result<serde_json::Value> {
        let Some(f) = &self.resolve else {
            return Ok(value);
        };
        let out: Value = f
            .call((to_lua(&self.lua, target)?, to_lua(&self.lua, &value)?))
            .map_err(clean_error)
            .with_context(|| format!("plugin `{}`", self.name))?;
        if out.is_nil() {
            bail!("plugin `{}`: `resolve` returned nothing", self.name);
        }
        from_lua(&self.lua, out)
            .with_context(|| format!("plugin `{}`: bad `resolve` result", self.name))
    }

    pub fn compile(&self, ctx: &Ctx) -> Result<Vec<Output>> {
        let Some(f) = &self.compile else {
            return Ok(Vec::new());
        };
        let out: Value = f
            .call(to_lua(&self.lua, ctx)?)
            .map_err(clean_error)
            .with_context(|| format!("plugin `{}`: compile failed", self.name))?;
        let raw: Vec<RawOutput> = if out.is_nil() {
            Vec::new()
        } else {
            from_lua(&self.lua, out).with_context(|| {
                format!(
                    "plugin `{}`: compile must return a list of {{ path = ..., content = ... }}",
                    self.name
                )
            })?
        };
        raw.into_iter()
            .map(|r| {
                let path = PathBuf::from(r.path);
                if !path.is_absolute() {
                    bail!(
                        "plugin `{}`: compile output `{}` is not an absolute path",
                        self.name,
                        path.display()
                    );
                }
                Ok(Output {
                    plugin: self.name.clone(),
                    path,
                    content: r.content,
                })
            })
            .collect()
    }

    /// Commands to run for `action`, each an argv list.
    pub fn package_commands(&self, action: Action, ctx: &Ctx) -> Result<Vec<Vec<String>>> {
        let f = match (&self.packages, action) {
            (Some(p), Action::Install) => p.install.as_ref(),
            (Some(p), Action::Upgrade) => p.upgrade.as_ref(),
            (None, _) => None,
        };
        let Some(f) = f else {
            bail!("plugin `{}` has no `packages.{}`", self.name, action.name());
        };
        let out: Value = f
            .call(to_lua(&self.lua, ctx)?)
            .map_err(clean_error)
            .with_context(|| {
                format!("plugin `{}`: packages.{} failed", self.name, action.name())
            })?;
        let cmds: Vec<Vec<String>> = if out.is_nil() {
            Vec::new()
        } else {
            from_lua(&self.lua, out).with_context(|| {
                format!(
                    "plugin `{}`: packages.{} must return a list of argv lists",
                    self.name,
                    action.name()
                )
            })?
        };
        if cmds.iter().any(Vec::is_empty) {
            bail!(
                "plugin `{}`: packages.{} returned an empty command",
                self.name,
                action.name()
            );
        }
        Ok(cmds)
    }

    /// Whatever `packages.list` returns, for printing.
    pub fn package_list(&self, ctx: &Ctx) -> Result<serde_json::Value> {
        let f = self.packages.as_ref().and_then(|p| p.list.as_ref());
        let Some(f) = f else {
            bail!("plugin `{}` has no `packages.list`", self.name);
        };
        let out: Value = f
            .call(to_lua(&self.lua, ctx)?)
            .map_err(clean_error)
            .with_context(|| format!("plugin `{}`: packages.list failed", self.name))?;
        from_lua(&self.lua, out)
            .with_context(|| format!("plugin `{}`: bad `packages.list` result", self.name))
    }
}

/// The loaded plugins, in declaration order.
pub struct Plugins {
    list: Vec<Plugin>,
}

impl std::fmt::Debug for Plugins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.list.iter().map(Plugin::name))
            .finish()
    }
}

impl Plugins {
    /// Build from the root file's `plugins` list (evaluated in `lua`, the
    /// root's VM), or the built-ins in a fresh VM when there is none.
    pub fn load(host: &Host, lua: Option<Lua>, declared: Option<Table>) -> Result<Plugins> {
        let lua = match lua {
            Some(l) => l,
            None => new_vm(host)?,
        };
        let tables: Vec<Table> = match declared {
            Some(list) => list
                .sequence_values::<Value>()
                .map(|v| match v? {
                    Value::Table(t) => Ok(t),
                    other => bail!(
                        "`plugins` entries must be tables, got {}",
                        other.type_name()
                    ),
                })
                .collect::<Result<_>>()?,
            None => {
                let require: Function = lua.globals().get("require")?;
                BUILTIN_PLUGINS
                    .iter()
                    .map(|(name, _)| require.call::<Table>(format!("qd.{name}")))
                    .collect::<mlua::Result<_>>()
                    .context("loading built-in plugins")?
            }
        };
        lua.remove_hook();

        let mut list: Vec<Plugin> = Vec::new();
        for t in tables {
            let p = Plugin::from_table(&lua, t)?;
            if list.iter().any(|q| q.name == p.name) {
                bail!("plugin `{}` is declared twice", p.name);
            }
            list.push(p);
        }
        Ok(Plugins { list })
    }

    pub fn iter(&self) -> impl Iterator<Item = &Plugin> {
        self.list.iter()
    }

    pub fn get(&self, name: &str) -> Option<&Plugin> {
        self.list.iter().find(|p| p.name == name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.list.iter().map(Plugin::name).collect()
    }

    /// Validate and normalise every plugin key of one file. Unknown keys are
    /// errors: neither a core field nor a loaded plugin.
    pub fn resolve(&self, target: &Target, ext: Ext) -> Result<Ext> {
        ext.into_iter()
            .map(|(key, value)| {
                let Some(plugin) = self.get(&key) else {
                    bail!(
                        "unknown field `{key}`: not a core field and no plugin named `{key}` is loaded (have: {})",
                        self.names().join(", ")
                    );
                };
                Ok((key, plugin.resolve(target, value)?))
            })
            .collect()
    }
}

fn check_keys(t: &Table, allowed: &[&str]) -> Result<()> {
    for pair in t.pairs::<Value, Value>() {
        let (k, _) = pair?;
        let ok = matches!(&k, Value::String(s) if s.to_str().map(|s| allowed.contains(&&*s)).unwrap_or(false));
        if !ok {
            bail!(
                "unknown key `{}` (allowed: {})",
                k.to_string().unwrap_or_default(),
                allowed.join(", ")
            );
        }
    }
    Ok(())
}

fn func(t: &Table, key: &str) -> Result<Option<Function>> {
    match t.get::<Value>(key)? {
        Value::Nil => Ok(None),
        Value::Function(f) => Ok(Some(f)),
        other => bail!("`{key}` must be a function, got {}", other.type_name()),
    }
}
