use std::path::{Path, PathBuf};
use std::process::Command;

use qd::compile;
use qd::crypto::Crypto;
use qd::host::{Host, Os};
use qd::packages;
use qd::repo::Repo;
use serde_json::json;

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

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

impl Env {
    fn new() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let home = tmp.path().join("home");
        let state = tmp.path().join("state");
        std::fs::create_dir_all(&home).unwrap();
        let (identity, recipient) = Crypto::generate();
        write(&repo.join("master.key"), &identity);
        write(&repo.join("master.rec"), &recipient);
        write(
            &repo.join("qd.lua"),
            r#"local qd = require("qd")
return { ignore = { "**/.DS_Store" }, nushell = { source = { qd.path.dotfiles("config.nu") } } }"#,
        );
        write(&repo.join("config.nu"), "# root\n");
        write(
            &repo.join("app/qd.lua"),
            r#"local qd = require("qd")
return {
  path = qd.path.home(".config/app"),
  brew = { "tool", { name = "extra", tap = "some/tap" } },
  scoop = { "tool", { name = "extra", bucket = "extras" } },
  encrypt = { "**/*.secret" },
  nushell = { source = { "config.nu" }, env_source = { "env.nu" } },
  setup = { version = 1, after = function(m) qd.write(qd.path.join(m.dest, "gen"), "g") end },
  ignore = { "**/gen" },
}"#,
        );
        write(&repo.join("app/config.nu"), "# app\n");
        write(&repo.join("app/env.nu"), "# env\n");
        write(&repo.join("app/a.txt"), "a1");
        write(
            &repo.join("other/qd.lua"),
            r#"local qd = require("qd")
return { enabled = not qd.tag("skip"), path = qd.path.home("other"), brew = { "tool", "second" } }"#,
        );
        write(&repo.join("other/o.txt"), "o1");
        write(
            &repo.join("nopath/qd.lua"),
            r#"return { brew = { "third" } }"#,
        );
        Env {
            _tmp: tmp,
            repo,
            home,
            state,
        }
    }

    fn qd(&self, args: &[&str]) -> (bool, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_qd"))
            .args(["--repo", self.repo.to_str().unwrap()])
            .args(args)
            .env("HOME", &self.home)
            .env("QD_STATE", &self.state)
            .env_remove("DOTFILES_TAGS")
            .env_remove("WSL_DISTRO_NAME")
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Like `qd`, but with extra environment variables set, so that the tests
    /// which care about detection can supply them deliberately.
    fn qd_env(&self, args: &[&str], env: &[(&str, &str)]) -> (bool, String, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_qd"));
        c.args(["--repo", self.repo.to_str().unwrap()])
            .args(args)
            .env("HOME", &self.home)
            .env("QD_STATE", &self.state)
            .env_remove("DOTFILES_TAGS")
            .env_remove("WSL_DISTRO_NAME");
        for (k, v) in env {
            c.env(k, v);
        }
        let out = c.output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (ok, out, err) = self.qd(args);
        assert!(ok, "qd {args:?} failed:\n{out}\n{err}");
        out
    }

    fn fail(&self, args: &[&str]) -> String {
        let (ok, out, err) = self.qd(args);
        assert!(!ok, "qd {args:?} unexpectedly succeeded:\n{out}");
        err
    }
}

#[test]
fn status_push_pull_undo_through_the_binary() {
    let e = Env::new();

    let out = e.ok(&["list"]);
    assert!(
        out.contains("app") && out.contains("other") && out.contains("nopath"),
        "{out}"
    );

    let out = e.ok(&["status"]);
    assert!(
        out.contains("[setup v1: destination does not exist]"),
        "{out}"
    );
    assert!(out.contains("copy     a.txt"), "{out}");

    let out = e.ok(&["push", "--dry-run"]);
    assert!(
        !e.home.join(".config/app/a.txt").exists(),
        "dry run wrote files: {out}"
    );

    let out = e.ok(&["push"]);
    assert!(out.contains("applied"), "{out}");
    assert!(
        out.contains("compile  ") && out.contains("compiled "),
        "compile step is planned and reported: {out}"
    );
    assert_eq!(read(&e.home.join(".config/app/a.txt")), "a1");
    assert_eq!(read(&e.home.join(".config/app/gen")), "g", "setup hook ran");
    assert_eq!(read(&e.home.join("other/o.txt")), "o1");
    let main = read(&e.home.join(".dotfiles.local.nu"));
    assert!(
        main.contains("source `") && main.contains("app/config.nu") && main.contains("config.nu`"),
        "{main}"
    );
    let env = read(&e.home.join(".dotfiles-env.local.nu"));
    assert!(env.contains("app/env.nu"), "{env}");

    let out = e.ok(&["status"]);
    assert!(out.contains("app                up to date"), "{out}");
    assert!(out.contains("compile            up to date"), "{out}");

    let state = read(&e.state.join("state.toml"));
    assert!(
        state.contains("[modules.app]") && state.contains("setup_version = 1"),
        "{state}"
    );

    write(&e.home.join(".config/app/a.txt"), "a2");
    write(&e.home.join(".config/app/k.secret"), "s");
    let out = e.ok(&["status", "--pull"]);
    assert!(out.contains("encrypt  repo:k.secret.age"), "{out}");
    e.ok(&["pull"]);
    assert_eq!(read(&e.repo.join("app/a.txt")), "a2");
    assert!(e.repo.join("app/k.secret.age").exists());
    let out = e.ok(&["status"]);
    assert!(out.contains("up to date"), "{out}");

    std::fs::remove_file(e.repo.join("app/a.txt")).unwrap();
    let out = e.ok(&["push", "--no-remove"]);
    assert!(
        out.contains("keep     a.txt (skipped, --no-remove)"),
        "{out}"
    );
    assert!(e.home.join(".config/app/a.txt").exists());
    let out = e.ok(&["push"]);
    assert!(out.contains("remove   a.txt"), "{out}");
    assert!(!e.home.join(".config/app/a.txt").exists());

    let out = e.ok(&["undo"]);
    assert!(out.contains("reverted run"), "{out}");
    assert_eq!(read(&e.home.join(".config/app/a.txt")), "a2");

    let out = e.ok(&["journal"]);
    assert!(out.contains("remove") && out.contains("restore"), "{out}");
    assert!(e.ok(&["trash", "path"]).trim().ends_with("trash"));
    assert!(
        e.ok(&["trash", "prune", "--older", "0s"])
            .contains("pruned")
    );
}

#[test]
fn tags_state_and_completion() {
    let e = Env::new();
    e.ok(&["tag", "add", "skip"]);
    assert_eq!(e.ok(&["tag", "list"]).trim(), "skip");
    let out = e.ok(&["list"]);
    assert!(
        out.contains("other              (disabled on this host)"),
        "{out}"
    );
    assert_eq!(e.ok(&["__complete", "modules"]).trim(), "app");
    e.ok(&["tag", "rm", "skip"]);
    assert_eq!(
        e.ok(&["__complete", "modules"]).lines().collect::<Vec<_>>(),
        ["app", "other"]
    );

    let err = e.fail(&["show", "missing"]);
    assert!(err.contains("no module named"), "{err}");
    let err = e.fail(&["push", "nopath"]);
    assert!(err.contains("cannot be synced"), "{err}");

    std::fs::create_dir_all(e.home.join(".config/app")).unwrap();
    let out = e.ok(&["state", "adopt"]);
    assert!(
        out.contains("adopted app") && out.contains("1 modules recorded"),
        "{out}"
    );
    let out = e.ok(&["state", "show"]);
    assert!(out.contains("setup_version = 1"), "{out}");
    let out = e.ok(&["status"]);
    assert!(
        !out.contains("setup v1"),
        "adopted module must not rerun setup: {out}"
    );
    assert!(out.contains("copy     a.txt"), "{out}");

    let out = e.ok(&["push", "--force", "--dry-run"]);
    assert!(out.contains("[setup v1: --force]"), "{out}");
}

#[test]
fn detected_facts_are_not_tags() {
    let e = Env::new();

    // WSL is a fact about the machine, readable through `qd.host`, and must not
    // leak into the tag set: a tag is something the machine declares, and one
    // that reappears on every run could never be removed.
    let (ok, out, err) = e.qd_env(&["host"], &[("WSL_DISTRO_NAME", "Ubuntu")]);
    assert!(ok, "{out}{err}");
    let h: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(h["wsl"], json!(true), "{out}");
    assert_eq!(h["tags"], json!([]), "wsl must not become a tag: {out}");

    // What the machine does declare still arrives, from either source.
    e.ok(&["tag", "add", "declared"]);
    let (ok, out, _) = e.qd_env(
        &["host"],
        &[("WSL_DISTRO_NAME", "Ubuntu"), ("DOTFILES_TAGS", "from_env")],
    );
    assert!(ok, "{out}");
    let h: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(h["tags"], json!(["declared", "from_env"]), "{out}");
}

#[test]
fn packages_fold_across_modules() {
    let e = Env::new();
    let repo = Repo::load(Host::new(Os::Darwin, &e.home, &e.repo)).unwrap();
    let brew = packages::pick(&repo, Some("brew")).unwrap();
    assert_eq!(
        packages::list(&repo, brew).unwrap(),
        json!({ "packages": ["tool", "extra", "third", "second"], "sources": ["some/tap"] }),
        "modules fold in name order"
    );
    let scoop = packages::pick(&repo, Some("scoop")).unwrap();
    assert_eq!(
        packages::list(&repo, scoop).unwrap(),
        json!({ "packages": ["main/tool", "extras/extra"], "sources": ["main", "extras"] })
    );
    let err = format!("{:#}", packages::pick(&repo, Some("nushell")).unwrap_err());
    assert!(err.contains("no `packages` section"), "{err}");

    let out = e.ok(&["packages", "list", "--manager", "brew"]);
    assert!(out.contains("\"some/tap\""), "{out}");
    let out = e.ok(&["packages", "list", "--manager", "brew", "--format", "toml"]);
    assert!(out.contains("sources = [\"some/tap\"]"), "{out}");
    let out = e.ok(&["packages", "install", "--manager", "brew", "--dry-run"]);
    assert!(
        out.contains("$ brew tap some/tap")
            && out.contains("$ brew install tool extra third second"),
        "{out}"
    );
    let out = e.ok(&["packages", "upgrade", "--manager", "scoop", "--dry-run"]);
    assert!(
        out.contains("$ scoop update main/tool extras/extra"),
        "{out}"
    );
}

#[test]
fn compile_orders_like_the_nushell_tool() {
    let e = Env::new();
    let repo = Repo::load(Host::new(Os::Darwin, &e.home, &e.repo)).unwrap();
    let outputs = compile::compile(&repo).unwrap();
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].plugin, "nushell");
    assert_eq!(outputs[0].path, e.home.join(".dotfiles.local.nu"));
    assert_eq!(
        outputs[0].content,
        format!(
            "source `{}`\nsource `{}`\n",
            e.repo.join("config.nu").display(),
            e.home.join(".config/app/config.nu").display()
        )
    );
    assert_eq!(outputs[1].path, e.home.join(".dotfiles-env.local.nu"));
    assert_eq!(
        outputs[1].content,
        format!("source `{}`\n", e.home.join(".config/app/env.nu").display())
    );
    assert_eq!(compile::changed(&outputs).len(), 2, "nothing written yet");
}

#[test]
fn self_update_reports_asset_name_and_handles_unreachable_api() {
    let name = qd::update::asset_name();
    assert!(name.starts_with("qd-"), "{name}");
    let src = qd::update::Source {
        host: "http://127.0.0.1:9".into(),
        owner: "x".into(),
        package: "qd".into(),
        token: None,
    };
    let err = qd::update::self_update(&src, true).unwrap_err();
    assert!(format!("{err:#}").contains("fetching"), "{err:#}");
}
