//! Plugins declared by the root `qd.lua`: resolve, compile, packages, and the
//! errors around them.

use std::path::{Path, PathBuf};
use std::process::Command;

use qd::compile;
use qd::host::{Host, Os};
use qd::journal::Journal;
use qd::packages::{self, Action};
use qd::repo::Repo;
use serde_json::json;

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

struct Env {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    state: PathBuf,
}

impl Env {
    fn new(root: &str) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let home = tmp.path().join("home");
        let state = tmp.path().join("state");
        std::fs::create_dir_all(&home).unwrap();
        write(&repo.join("qd.lua"), root);
        write(
            &repo.join("plugins/aliases.lua"),
            r#"
local qd = require("qd")
return {
  name = "aliases",
  resolve = function(m, value)
    if m.root then qd.fail("`aliases` belongs in modules") end
    qd.check(value, qd.schema.map(qd.schema.string()), "aliases")
    local out = {}
    for k, v in pairs(value) do out[k] = m.name .. ":" .. v end
    return out
  end,
  compile = function(ctx)
    local lines = {}
    for _, m in ipairs(ctx.modules) do
      for k, v in pairs(m.aliases or {}) do lines[#lines + 1] = "alias " .. k .. " = " .. v end
    end
    table.sort(lines)
    return { { path = qd.path.home(".aliases"), content = table.concat(lines, "\n") .. "\n" } }
  end,
  packages = {
    list = function(ctx) return { count = #ctx.modules } end,
    install = function(ctx)
      local c = { "echo", "install" }
      for _, m in ipairs(ctx.modules) do c[#c + 1] = m.name end
      return { c }
    end,
  },
}
"#,
        );
        write(
            &repo.join("app/qd.lua"),
            r#"local qd = require("qd")
return { path = qd.path.home("app"), aliases = { ll = "ls -l" } }"#,
        );
        write(
            &repo.join("zed/qd.lua"),
            r#"return { aliases = { g = "git" } }"#,
        );
        Env {
            _tmp: tmp,
            repo,
            home,
            state,
        }
    }

    fn load(&self) -> anyhow::Result<Repo> {
        Repo::load(Host::new(Os::Darwin, &self.home, &self.repo))
    }

    fn err(&self) -> String {
        format!("{:#}", self.load().unwrap_err())
    }
}

const CUSTOM_ONLY: &str = r#"return { plugins = { require("plugins.aliases") } }"#;

#[test]
fn declared_plugins_replace_the_builtins() {
    let e = Env::new(CUSTOM_ONLY);
    let repo = e.load().unwrap();
    assert_eq!(repo.plugins.names(), ["aliases"]);
    assert!(repo.plugins.get("nushell").is_none());

    let app = repo.module("app").unwrap();
    assert_eq!(
        app.ext["aliases"],
        json!({ "ll": "app:ls -l" }),
        "resolve ran"
    );
    assert_eq!(app.view().ext["aliases"], json!({ "ll": "app:ls -l" }));
}

#[test]
fn compile_writes_and_journals_under_the_run() {
    let e = Env::new(CUSTOM_ONLY);
    let repo = e.load().unwrap();
    let outputs = compile::compile(&repo).unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].path, e.home.join(".aliases"));
    assert_eq!(
        outputs[0].content,
        "alias g = zed:git\nalias ll = app:ls -l\n"
    );

    let journal = Journal::new(&e.state);
    let written = compile::apply(&repo, &journal, "run-1").unwrap();
    assert_eq!(written, [e.home.join(".aliases")]);
    assert_eq!(
        std::fs::read_to_string(e.home.join(".aliases")).unwrap(),
        "alias g = zed:git\nalias ll = app:ls -l\n"
    );
    assert!(compile::changed(&outputs).is_empty(), "converged");
    assert!(compile::apply(&repo, &journal, "run-2").unwrap().is_empty());

    let entries = journal.read_all().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].run.as_str(), entries[0].op.as_str()),
        ("run-1", "compile")
    );
    assert_eq!(entries[0].module, "aliases");
    assert!(entries[0].created);

    // Overwrite by hand, recompile, undo: the hand-written version comes back.
    write(&e.home.join(".aliases"), "mine\n");
    compile::apply(&repo, &journal, "run-3").unwrap();
    journal.undo("run-3").unwrap();
    assert_eq!(
        std::fs::read_to_string(e.home.join(".aliases")).unwrap(),
        "mine\n"
    );
}

#[test]
fn package_plugins_return_commands() {
    let e = Env::new(CUSTOM_ONLY);
    let repo = e.load().unwrap();
    let p = packages::pick(&repo, None).unwrap();
    assert_eq!(p.name(), "aliases");
    assert_eq!(packages::list(&repo, p).unwrap(), json!({ "count": 2 }));
    assert_eq!(
        p.package_commands(Action::Install, &repo.ctx()).unwrap(),
        [["echo", "install", "app", "zed"]]
    );
    let err = format!(
        "{:#}",
        p.package_commands(Action::Upgrade, &repo.ctx())
            .unwrap_err()
    );
    assert!(err.contains("no `packages.upgrade`"), "{err}");
}

/// `packages.list` may return anything, a bare list included, so the default
/// output format has to be one that can represent it.
#[test]
fn a_list_returning_plugin_prints() {
    let e = Env::new(
        r#"return { plugins = { { name = "apt", packages = { list = function(ctx)
             local out = {}
             for _, m in ipairs(ctx.modules) do out[#out + 1] = m.name end
             return out
           end } } } }"#,
    );
    write(&e.repo.join("app/qd.lua"), "return {}");
    write(&e.repo.join("zed/qd.lua"), "return {}");
    let repo = e.load().unwrap();
    let p = packages::pick(&repo, None).unwrap();
    assert_eq!(packages::list(&repo, p).unwrap(), json!(["app", "zed"]));

    let out = Command::new(env!("CARGO_BIN_EXE_qd"))
        .args(["--repo", e.repo.to_str().unwrap(), "packages", "list"])
        .env("HOME", &e.home)
        .env("QD_STATE", &e.state)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("\"app\""),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn builtins_can_be_mixed_in_and_ordered() {
    let e = Env::new(
        r#"return { plugins = { require("plugins.aliases"), require("qd.nushell") },
                   nushell = { source = { qd.path.dotfiles("config.nu") } } }"#,
    );
    let repo = e.load().unwrap();
    assert_eq!(repo.plugins.names(), ["aliases", "nushell"]);
    let outputs = compile::compile(&repo).unwrap();
    let plugins: Vec<&str> = outputs.iter().map(|o| o.plugin.as_str()).collect();
    assert_eq!(plugins, ["aliases", "nushell", "nushell"]);
}

#[test]
fn keys_without_a_plugin_are_errors() {
    let e = Env::new(r#"return {}"#);
    let err = e.err();
    assert!(
        err.contains("module `app`")
            && err.contains("unknown field `aliases`")
            && err.contains("have: nushell, brew, scoop"),
        "{err}"
    );
}

#[test]
fn resolve_errors_carry_module_and_plugin() {
    let e =
        Env::new(r#"return { plugins = { require("plugins.aliases") }, aliases = { x = "y" } }"#);
    let err = e.err();
    assert!(
        err.contains("plugin `aliases`") && err.contains("belongs in modules"),
        "{err}"
    );

    let e = Env::new(CUSTOM_ONLY);
    write(&e.repo.join("zed/qd.lua"), r#"return { aliases = "nope" }"#);
    let err = e.err();
    assert!(
        err.contains("module `zed`") && err.contains("must be a table"),
        "{err}"
    );
}

#[test]
fn plugin_tables_are_validated() {
    for (root, expect) in [
        (
            r#"return { plugins = { { compile = function() end } } }"#,
            "without a string `name`",
        ),
        (
            r#"return { plugins = { { name = "path" } } }"#,
            "collides with a core field",
        ),
        (
            r#"return { plugins = { { name = "x", compiel = 1 } } }"#,
            "unknown key `compiel`",
        ),
        (
            r#"return { plugins = { { name = "x", compile = 1 } } }"#,
            "`compile` must be a function",
        ),
        (
            r#"return { plugins = { { name = "x" }, { name = "x" } } }"#,
            "declared twice",
        ),
        (r#"return { plugins = { 1 } }"#, "entries must be tables"),
        (r#"return { plugins = 1 }"#, "`plugins` must be a list"),
    ] {
        let e = Env::new(root);
        write(&e.repo.join("app/qd.lua"), "return {}");
        write(&e.repo.join("zed/qd.lua"), "return {}");
        let err = e.err();
        assert!(err.contains(expect), "{root}\n{err}");
    }
}

#[test]
fn plugins_only_in_the_root() {
    let e = Env::new(CUSTOM_ONLY);
    write(&e.repo.join("zed/qd.lua"), r#"return { plugins = {} }"#);
    let err = e.err();
    assert!(err.contains("only be declared in the root"), "{err}");
}

#[test]
fn compile_results_are_checked() {
    let bad = |body: &str| {
        let e = Env::new(&format!(
            r#"return {{ plugins = {{ {{ name = "x", compile = function(ctx) {body} end }} }} }}"#
        ));
        write(&e.repo.join("app/qd.lua"), "return {}");
        write(&e.repo.join("zed/qd.lua"), "return {}");
        let repo = e.load().unwrap();
        format!("{:#}", compile::compile(&repo).unwrap_err())
    };
    let err = bad(r#"return { { path = "relative", content = "" } }"#);
    assert!(err.contains("not an absolute path"), "{err}");
    let err = bad(r#"return { { path = "/x", contents = "" } }"#);
    assert!(err.contains("must return a list of"), "{err}");
    let err = bad(r#"qd.write("/x", "y")"#);
    assert!(
        err.contains("setup.before or setup.after"),
        "plugins stay pure: {err}"
    );

    let e = Env::new(
        r#"local one = function() return { { path = "/same", content = "" } } end
           return { plugins = { { name = "a", compile = one }, { name = "b", compile = one } } }"#,
    );
    write(&e.repo.join("app/qd.lua"), "return {}");
    write(&e.repo.join("zed/qd.lua"), "return {}");
    let err = format!("{:#}", compile::compile(&e.load().unwrap()).unwrap_err());
    assert!(err.contains("both compile /same"), "{err}");
}

#[test]
fn which_finds_executables_on_path() {
    let tmp = tempfile::tempdir().unwrap();
    let e = Env::new(CUSTOM_ONLY);
    write(
        &e.repo.join("app/qd.lua"),
        r#"return { path = qd.path.home("app") }"#,
    );
    write(
        &e.repo.join("zed/qd.lua"),
        r#"assert(qd.which("definitely-not-a-command-qd") == nil)
           assert(qd.which("sh") ~= nil or qd.which("cmd") ~= nil)
           assert(qd.path.is_absolute("/x") and not qd.path.is_absolute("x"))
           return {}"#,
    );
    drop(tmp);
    e.load().unwrap();
}
