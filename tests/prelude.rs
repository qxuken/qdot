//! The Lua helpers every config and plugin gets: `qd.fail`, `qd.check`, and
//! the logging that must never reach stdout.

use std::path::{Path, PathBuf};
use std::process::Command;

use qd::host::{Host, Os};
use qd::repo::Repo;

struct Env {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    state: PathBuf,
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

impl Env {
    /// A repo with one module whose config is `body`.
    fn new(body: &str) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let home = tmp.path().join("home");
        let state = tmp.path().join("state");
        std::fs::create_dir_all(&home).unwrap();
        write(
            &repo.join("m/qd.lua"),
            &format!("local qd = require(\"qd\")\n{body}\n"),
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

/// Everything valid, asserted inside Lua: loading at all means it passed.
#[test]
fn schema_accepts_and_returns_the_value() {
    Env::new(
        r#"
local s = qd.schema
assert(qd.check("x", s.string(), "a") == "x")
assert(qd.check(1, s.number(), "a") == 1)
assert(qd.check(true, s.boolean(), "a"))
assert(qd.check(nil, s.optional(s.string()), "a") == nil)
assert(qd.check("x", s.optional(s.string()), "a") == "x")
assert(qd.check({ "a", "b" }, s.list(s.string()), "a")[2] == "b")
assert(#qd.check({}, s.list(s.string()), "a") == 0)
assert(qd.check(nil, s.any(), "a") == nil)
assert(qd.check({ n = "x" }, s.table { n = s.string(), t = s.optional(s.string()) }, "a").n == "x")
assert(qd.check({ n = { 1 } }, s.record { n = s.list(s.number()) }, "a").n[1] == 1)
assert(qd.check({ ll = "ls -l" }, s.map(s.string()), "a").ll == "ls -l")
assert(qd.check({}, s.map(s.string()), "a") ~= nil)
assert(qd.check("always", s.enum("auto", "always"), "a") == "always")

-- a union accepts either form
local entry = s.one_of(s.string(), s.table { name = s.string() })
assert(qd.check("git", entry, "a") == "git")
assert(qd.check({ name = "git" }, entry, "a").name == "git")
assert(qd.check({ "git", { name = "fd" } }, s.list(entry), "a")[2].name == "fd")

-- a plain function is a validator, so plugins can write their own
local even = function(v, path)
  if v % 2 ~= 0 then qd.fail("`%s` must be even", path) end
  return v
end
assert(qd.check(4, even, "a") == 4)
assert(qd.check({ 2, 4 }, s.list(even), "a")[1] == 2)
return {}
"#,
    )
    .load()
    .unwrap();
}

#[test]
fn schema_rejects_and_names_the_field() {
    for (body, expect) in [
        (
            "qd.check(1, s.string(), 'a')",
            "`a` must be a string, got number",
        ),
        (
            "qd.check(nil, s.string(), 'a')",
            "`a` must be a string, got nil",
        ),
        (
            "qd.check('x', s.list(s.string()), 'a')",
            "`a` must be a list, got string",
        ),
        (
            "qd.check({ k = 1 }, s.list(s.number()), 'a')",
            "`a` must be a list, got table",
        ),
        (
            "qd.check({ 'x', 2 }, s.list(s.string()), 'a')",
            "`a[2]` must be a string, got number",
        ),
        (
            "qd.check({ b = 1 }, s.table { c = s.number() }, 'a')",
            "unknown field `a.b`",
        ),
        (
            "qd.check({}, s.table { c = s.number() }, 'a')",
            "`a.c` must be a number, got nil",
        ),
        (
            "qd.check({ c = { d = 'x' } }, s.table { c = s.table { d = s.number() } }, 'a')",
            "`a.c.d` must be a number, got string",
        ),
        (
            "qd.check({ [1] = 'x' }, s.map(s.string()), 'a')",
            "`a` keys must be strings, got number",
        ),
        (
            "qd.check({ k = 1 }, s.map(s.string()), 'a')",
            "`a.k` must be a string, got number",
        ),
        (
            "qd.check('never', s.enum('auto', 'always'), 'a')",
            "`a` must be `auto` or `always`, got `never`",
        ),
        // A spec mistake is reported against the plugin, not the config.
        (
            "qd.check('x', 'string', 'a')",
            "qd.check: expected a validator from qd.schema, got string",
        ),
        (
            "qd.check('x', s.list('string'), 'a')",
            "qd.schema.list: expected a validator",
        ),
        (
            "qd.check('x', s.table { c = 1 }, 'a')",
            "qd.schema.table field `c`: expected a validator",
        ),
        (
            "qd.check('x', s.one_of(s.string()), 'a')",
            "one_of: needs at least two alternatives",
        ),
    ] {
        let err = Env::new(&format!("local s = qd.schema\n{body}\nreturn {{}}")).err();
        assert!(err.contains(expect), "{body}\nwanted {expect:?}, got {err}");
    }
}

/// A union should not degrade the message when only one alternative could
/// have been meant.
#[test]
fn union_errors_stay_specific() {
    let entry = "s.one_of(s.string(), s.table { name = s.string(), tap = s.optional(s.string()) })";
    for (value, expect) in [
        // Only the record accepts a table, so its own error survives.
        ("{ nme = 'x' }", "unknown field `a.nme`"),
        ("{ name = 1 }", "`a.name` must be a string, got number"),
        (
            "{ name = 'x', tap = 2 }",
            "`a.tap` must be a string, got number",
        ),
        // Nothing accepts a number, so both forms are listed.
        ("7", "`a` must be a string or a table, got number"),
    ] {
        let err = Env::new(&format!(
            "local s = qd.schema\nqd.check({value}, {entry}, 'a')\nreturn {{}}"
        ))
        .err();
        assert!(
            err.contains(expect),
            "{value}\nwanted {expect:?}, got {err}"
        );
    }
}

/// A config error should read as a sentence, not as a Lua diagnostic.
#[test]
fn fail_formats_and_drops_the_source_position() {
    let err = Env::new("qd.fail(\"bad %s (%d)\", \"thing\", 2)\nreturn {}").err();
    assert!(err.contains("bad thing (2)"), "{err}");
    assert!(!err.contains("qd.lua:2:"), "no file:line prefix: {err}");

    // With no arguments the string is used as-is, so a literal % survives.
    let err = Env::new("qd.fail(\"100% done\")\nreturn {}").err();
    assert!(err.contains("100% done"), "{err}");

    let err = Env::new("qd.fail({})\nreturn {}").err();
    assert!(err.contains("expected a format string, got table"), "{err}");
}

/// stdout carries `show --format json` and the completion lists, so nothing a
/// config says may land there.
#[test]
fn config_output_goes_to_stderr_only() {
    let e = Env::new(
        r#"
print("printed", 1)
qd.warn("warned %s", "once")
qd.debug("debugged")
return { path = qd.path.home("m") }
"#,
    );
    let run = |debug: bool| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_qd"));
        c.args(["--repo", e.repo.to_str().unwrap(), "show", "m"])
            .env("HOME", &e.home)
            .env("QD_STATE", &e.state)
            .env_remove("QD_DEBUG");
        if debug {
            c.env("QD_DEBUG", "1");
        }
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "{:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    let (stdout, stderr) = run(false);
    serde_json::from_str::<serde_json::Value>(&stdout).expect("stdout stays valid JSON");
    assert!(
        stderr.contains("printed\t1"),
        "print reaches stderr: {stderr}"
    );
    assert!(stderr.contains("qd: warning: warned once"), "{stderr}");
    assert!(
        !stderr.contains("debugged"),
        "debug is off by default: {stderr}"
    );

    let (_, stderr) = run(true);
    assert!(stderr.contains("qd: debug: debugged"), "{stderr}");
}

/// The example in contrib/ has to keep working: it is what a new package
/// manager gets copied from.
#[test]
fn the_contrib_apt_example_loads_and_folds() {
    let e = Env::new(r#"return { path = qd.path.home("m"), apt = { "ripgrep", "fd" } }"#);
    let apt = Path::new(env!("CARGO_MANIFEST_DIR")).join("contrib/apt.lua");
    write(
        &e.repo.join("plugins/apt.lua"),
        &std::fs::read_to_string(apt).unwrap(),
    );
    write(
        &e.repo.join("qd.lua"),
        r#"return { plugins = { require("qd.brew"), require("plugins.apt") } }"#,
    );
    write(
        &e.repo.join("n/qd.lua"),
        r#"return { apt = { "fd", "jq" } }"#,
    );

    let repo = e.load().unwrap();
    let apt = repo.plugins.get("apt").unwrap();
    assert_eq!(
        qd::packages::list(&repo, apt).unwrap(),
        serde_json::json!(["ripgrep", "fd", "jq"]),
        "deduped in module order"
    );
    assert_eq!(
        apt.package_commands(qd::packages::Action::Install, &repo.ctx())
            .unwrap(),
        [
            vec!["sudo", "apt-get", "update"],
            vec!["sudo", "apt-get", "install", "-y", "ripgrep", "fd", "jq"]
        ]
    );

    write(&e.repo.join("n/qd.lua"), r#"return { apt = { 1 } }"#);
    let err = e.err();
    assert!(
        err.contains("`apt[1]` must be a string, got number"),
        "{err}"
    );
}
