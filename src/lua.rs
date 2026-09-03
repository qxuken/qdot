//! The Lua host: a sandboxed VM per config file plus the `qd` API table.
//!
//! Two phases share one VM. During *load* the file is evaluated to a table and
//! only pure helpers are usable. During *hook* the `setup.before` / `setup.after`
//! functions run and `qd.run`, `qd.exec`, `qd.write` become callable.

use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use mlua::serde::de::Options as DeOptions;
use mlua::{
    Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value, Variadic, VmState,
};
use serde::Serialize;

use crate::config::RawModule;
use crate::host::Host;

/// Instruction budget for evaluating one config file. Generous for config,
/// far too small for an accidental infinite loop.
const LOAD_BUDGET: u64 = 5_000_000;
const HOOK_GRANULARITY: u32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Load,
    Hook,
}

pub struct Loaded {
    pub raw: RawModule,
    pub hooks: Hooks,
}

/// Owns the VM a module was loaded in, so its hook functions stay callable.
pub struct Hooks {
    lua: Lua,
    before: Option<Function>,
    after: Option<Function>,
}

impl Hooks {
    pub fn has_before(&self) -> bool {
        self.before.is_some()
    }

    pub fn has_after(&self) -> bool {
        self.after.is_some()
    }

    pub fn run_before<A: Serialize>(&self, arg: &A) -> Result<()> {
        self.call(self.before.as_ref(), "setup.before", arg)
    }

    pub fn run_after<A: Serialize>(&self, arg: &A) -> Result<()> {
        self.call(self.after.as_ref(), "setup.after", arg)
    }

    fn call<A: Serialize>(&self, f: Option<&Function>, what: &str, arg: &A) -> Result<()> {
        let Some(f) = f else { return Ok(()) };
        let arg = self.lua.to_value(arg)?;
        self.lua.set_app_data(Phase::Hook);
        let r = f.call::<()>(arg).with_context(|| format!("{what} failed"));
        self.lua.set_app_data(Phase::Load);
        r
    }
}

/// Evaluate `path` (a `qd.lua`) in a fresh sandbox and split the result into
/// serde data plus hook functions.
pub fn load_file(host: &Host, path: &Path) -> Result<Loaded> {
    let code =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let lua = new_vm(host)?;

    let value: Value = lua
        .load(&code)
        .set_name(format!("@{}", path.display()))
        .eval()
        .with_context(|| format!("evaluating {}", path.display()))?;
    lua.remove_hook();

    let table = match value {
        Value::Table(t) => t,
        other => bail!(
            "{}: expected the file to return a table, got {}",
            path.display(),
            other.type_name()
        ),
    };

    let (before, after) =
        take_hooks(&table).with_context(|| format!("{}: bad `setup`", path.display()))?;

    let raw: RawModule = lua
        .from_value_with(
            Value::Table(table),
            DeOptions::new().deny_unsupported_types(false),
        )
        .with_context(|| format!("{}: invalid config", path.display()))?;

    Ok(Loaded {
        raw,
        hooks: Hooks { lua, before, after },
    })
}

fn take_hooks(table: &Table) -> Result<(Option<Function>, Option<Function>)> {
    let setup: Value = table.get("setup")?;
    let Value::Table(setup) = setup else {
        return Ok((None, None));
    };
    let mut out = [None, None];
    for (i, key) in ["before", "after"].into_iter().enumerate() {
        match setup.get::<Value>(key)? {
            Value::Nil => {}
            Value::Function(f) => {
                out[i] = Some(f);
                setup.set(key, Value::Nil)?;
            }
            other => bail!(
                "`setup.{key}` must be a function, got {}",
                other.type_name()
            ),
        }
    }
    let [before, after] = out;
    Ok((before, after))
}

fn new_vm(host: &Host) -> Result<Lua> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )?;
    lua.set_app_data(Phase::Load);

    let g = lua.globals();
    for name in ["dofile", "loadfile", "load", "require", "collectgarbage"] {
        g.set(name, Value::Nil)?;
    }

    let qd = build_qd(&lua, host)?;
    g.set("qd", qd.clone())?;
    g.set("require", make_require(&lua, host.dotfiles.clone(), qd)?)?;

    let spent = Arc::new(AtomicU64::new(0));
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_GRANULARITY),
        move |_, _| {
            let n = spent.fetch_add(HOOK_GRANULARITY as u64, Ordering::Relaxed);
            if n > LOAD_BUDGET {
                return Err(mlua::Error::runtime(
                    "config evaluation exceeded its instruction budget",
                ));
            }
            Ok(VmState::Continue)
        },
    )?;

    Ok(lua)
}

/// `require("qd")` returns the API table; `require("name")` loads
/// `<repo>/name.lua` (dots become separators) once and caches it.
fn make_require(lua: &Lua, repo: PathBuf, qd: Table) -> Result<Function> {
    let cache = lua.create_table()?;
    cache.set("qd", qd)?;
    Ok(lua.create_function(move |lua, name: String| {
        if let Value::Table(t) = cache.get::<Value>(name.as_str())? {
            return Ok(Value::Table(t));
        }
        let rel: PathBuf = name.split('.').collect();
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(mlua::Error::runtime(format!(
                "require: invalid module name `{name}`"
            )));
        }
        let file = repo.join(rel).with_extension("lua");
        let code = std::fs::read_to_string(&file).map_err(|e| {
            mlua::Error::runtime(format!("require: cannot read {}: {e}", file.display()))
        })?;
        let value: Value = lua
            .load(&code)
            .set_name(format!("@{}", file.display()))
            .eval()?;
        cache.set(name.as_str(), value.clone())?;
        Ok(value)
    })?)
}

fn build_qd(lua: &Lua, host: &Host) -> Result<Table> {
    let qd = lua.create_table()?;

    let h = lua.create_table()?;
    h.set("name", host.os.name())?;
    h.set("darwin", host.os == crate::host::Os::Darwin)?;
    h.set("linux", host.os == crate::host::Os::Linux)?;
    h.set("windows", host.os == crate::host::Os::Windows)?;
    h.set("posix", host.os.is_posix())?;
    h.set("wsl", host.wsl)?;
    h.set("distro", host.distro.clone())?;
    h.set("ubuntu", host.distro.as_deref() == Some("ubuntu"))?;
    qd.set("host", h)?;

    let tags = host.tags.clone();
    qd.set(
        "tag",
        lua.create_function(move |_, name: String| Ok(tags.contains(&name)))?,
    )?;

    let path = lua.create_table()?;
    path_fn(lua, &path, "home", Some(host.home.clone()))?;
    path_fn(lua, &path, "config", Some(host.config.clone()))?;
    path_fn(lua, &path, "cache", Some(host.cache.clone()))?;
    path_fn(lua, &path, "app_support", Some(host.app_support.clone()))?;
    path_fn(lua, &path, "dotfiles", Some(host.dotfiles.clone()))?;
    path_fn(lua, &path, "appdata", host.appdata.clone())?;
    path_fn(lua, &path, "local_appdata", host.local_appdata.clone())?;
    path.set(
        "join",
        lua.create_function(|_, parts: Variadic<String>| {
            let mut it = parts.into_iter();
            let Some(first) = it.next() else {
                return Err(mlua::Error::runtime(
                    "qd.path.join needs at least one segment",
                ));
            };
            Ok(path_string(it.fold(PathBuf::from(first), |p, s| p.join(s))))
        })?,
    )?;
    qd.set("path", path)?;

    qd.set(
        "list",
        lua.create_function(|lua, (base, extra): (Table, Variadic<Value>)| {
            let t = lua.create_table()?;
            for v in base.sequence_values::<Value>() {
                t.push(v?)?;
            }
            for v in extra {
                t.push(v)?;
            }
            Ok(t)
        })?,
    )?;

    qd.set(
        "env",
        lua.create_function(|_, name: String| Ok(std::env::var(name).ok()))?,
    )?;
    qd.set(
        "exists",
        lua.create_function(|_, p: String| Ok(Path::new(&p).exists()))?,
    )?;

    qd.set(
        "run",
        lua.create_function(|lua, (cmd, args): (String, Variadic<String>)| {
            ensure_hook_phase(lua, "run")?;
            let status = Command::new(&cmd)
                .args(args.iter())
                .status()
                .map_err(|e| mlua::Error::runtime(format!("qd.run: cannot start `{cmd}`: {e}")))?;
            if !status.success() {
                return Err(mlua::Error::runtime(format!(
                    "qd.run: `{cmd}` exited with {status}"
                )));
            }
            Ok(())
        })?,
    )?;

    qd.set(
        "exec",
        lua.create_function(|lua, (cmd, args): (String, Variadic<String>)| {
            ensure_hook_phase(lua, "exec")?;
            let out = Command::new(&cmd)
                .args(args.iter())
                .output()
                .map_err(|e| mlua::Error::runtime(format!("qd.exec: cannot start `{cmd}`: {e}")))?;
            if !out.status.success() {
                return Err(mlua::Error::runtime(format!(
                    "qd.exec: `{cmd}` exited with {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        })?,
    )?;

    qd.set(
        "write",
        lua.create_function(|lua, (p, content): (String, String)| {
            ensure_hook_phase(lua, "write")?;
            let p = PathBuf::from(p);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    mlua::Error::runtime(format!(
                        "qd.write: cannot create {}: {e}",
                        parent.display()
                    ))
                })?;
            }
            std::fs::write(&p, content)
                .map_err(|e| mlua::Error::runtime(format!("qd.write: {}: {e}", p.display())))?;
            Ok(())
        })?,
    )?;

    Ok(qd)
}

fn path_fn(lua: &Lua, table: &Table, name: &'static str, base: Option<PathBuf>) -> Result<()> {
    let f = lua.create_function(move |_, parts: Variadic<String>| {
        let Some(base) = base.as_ref() else {
            return Err(mlua::Error::runtime(format!(
                "qd.path.{name} is not available on this host"
            )));
        };
        Ok(path_string(
            parts.iter().fold(base.clone(), |p, s| p.join(s)),
        ))
    })?;
    table.set(name, f)?;
    Ok(())
}

fn path_string(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

fn ensure_hook_phase(lua: &Lua, fname: &str) -> mlua::Result<()> {
    match lua.app_data_ref::<Phase>().map(|p| *p) {
        Some(Phase::Hook) => Ok(()),
        _ => Err(mlua::Error::runtime(format!(
            "qd.{fname} can only be called from setup.before or setup.after"
        ))),
    }
}
