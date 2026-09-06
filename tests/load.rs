use std::path::{Path, PathBuf};

use qd::host::{Host, Os};
use qd::repo::Repo;
use serde_json::json;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn host(tags: &[&str]) -> Host {
    let mut h = Host::new(Os::Darwin, "/h", fixture("repo1"));
    h.tags = tags.iter().map(|s| s.to_string()).collect();
    h
}

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn loads_all_modules_and_root() {
    let repo = Repo::load(host(&[])).unwrap();
    let names: Vec<&str> = repo.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        ["neovim", "starship", "version-control", "vpn", "wezterm"]
    );
    assert!(repo.disabled.is_empty());

    assert_eq!(repo.global.ignore, ["**/.git/**", "**/.DS_Store"]);
    assert_eq!(
        repo.global.include,
        [fixture("repo1").join(".editorconfig")]
    );
    assert_eq!(
        repo.global.ext["nushell"]["source"],
        json!([fixture("repo1").join("config.nu")])
    );
    assert_eq!(repo.plugins.names(), ["nushell", "brew", "scoop"]);
}

#[test]
fn neovim_resolves_paths_packages_and_nushell() {
    let repo = Repo::load(host(&[])).unwrap();
    let m = repo.module("neovim").unwrap();
    assert_eq!(m.dest.as_deref(), Some(p("/h/.config/nvim").as_path()));
    assert_eq!(m.src, fixture("repo1").join("neovim"));

    assert_eq!(
        m.ext["brew"],
        json!(["git", "fzf", "neovim"]),
        "lib.lua via require plus qd.list append"
    );
    assert_eq!(m.ext["brew"], m.ext["scoop"]);

    let nu = &m.ext["nushell"];
    assert_eq!(nu["source"], json!(["/h/.config/nvim/config.nu"]));
    assert_eq!(nu["env_source"], json!(["/h/.config/nvim/env.nu"]));
    assert_eq!(nu["include"], json!([]));

    let setup = m.setup.as_ref().expect("setup present");
    assert_eq!((setup.version, setup.before, setup.after), (1, false, true));
}

#[test]
fn root_globs_are_prepended_and_compiled() {
    let repo = Repo::load(host(&[])).unwrap();
    let m = repo.module("vpn").unwrap();
    assert_eq!(m.ignore, ["**/.git/**", "**/.DS_Store", "**/history.txt"]);
    assert_eq!(m.encrypt, ["**/*.p12", "**/config.yml"]);
    assert!(m.encrypt_set.is_match("germany.p12"));
    assert!(m.encrypt_set.is_match("deep/er/config.yml"));
    assert!(!m.encrypt_set.is_match("config.yaml"));
    assert!(m.ignore_set.is_match(".git/HEAD"));
    assert!(m.ignore_set.is_match("sub/.git/HEAD"));
    assert_eq!(m.include, [fixture("repo1").join(".editorconfig")]);
    assert_eq!(m.ext["nushell"]["include"], json!(["/h/vpn/vpn.nu"]));
    assert!(m.setup.is_none());
}

#[test]
fn windows_host_picks_local_appdata() {
    let mut h = Host::new(Os::Windows, "/win/home", fixture("repo1"));
    h.local_appdata = Some(p("/win/home/AppData/Local"));
    let repo = Repo::load(h).unwrap();
    let m = repo.module("neovim").unwrap();
    assert_eq!(
        m.dest.as_deref(),
        Some(p("/win/home/AppData/Local/nvim").as_path())
    );
}

#[test]
fn ubuntu_branch_appends_packages() {
    let mut h = host(&[]);
    h.os = Os::Linux;
    h.distro = Some("ubuntu".into());
    let repo = Repo::load(h).unwrap();
    assert_eq!(
        repo.module("neovim").unwrap().ext["brew"],
        json!(["git", "fzf", "neovim", "xclip"])
    );
}

#[test]
fn tags_disable_modules_and_file_pairs() {
    let repo = Repo::load(host(&["wsl", "work"])).unwrap();
    assert_eq!(repo.disabled, ["wezterm"]);
    let err = repo.module("wezterm").unwrap_err().to_string();
    assert!(err.contains("disabled"), "{err}");

    let vc = repo.module("version-control").unwrap();
    assert_eq!(vc.files.len(), 1);
    assert_eq!(
        vc.files[0].src,
        fixture("repo1").join("version-control/always")
    );
    assert_eq!(vc.files[0].dest, p("/h/.always"));

    let repo = Repo::load(host(&[])).unwrap();
    let vc = repo.module("version-control").unwrap();
    assert_eq!(vc.files.len(), 2);
    assert_eq!(vc.files[0].dest, p("/h/.gitconfig"));
}

#[test]
fn detailed_packages_and_setup_version() {
    let repo = Repo::load(host(&[])).unwrap();
    let m = repo.module("starship").unwrap();
    assert_eq!(
        m.ext["brew"],
        json!([{ "name": "starship", "tap": "some/tap" }])
    );
    assert_eq!(
        m.ext["scoop"],
        json!([{ "name": "starship", "bucket": "main" }])
    );
    let setup = m.setup.as_ref().unwrap();
    assert_eq!((setup.version, setup.before, setup.after), (3, true, true));
}

#[test]
fn unknown_field_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("broken")).unwrap();
    std::fs::write(
        tmp.path().join("broken/qd.lua"),
        "return { path = '/tmp/x', typo_field = true }",
    )
    .unwrap();
    let err = Repo::load(Host::new(Os::Darwin, "/h", tmp.path())).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("module `broken`") && msg.contains("unknown field `typo_field`"),
        "{msg}"
    );
}

#[test]
fn relative_path_without_module_path_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("m")).unwrap();
    std::fs::write(
        tmp.path().join("m/qd.lua"),
        "return { nushell = { source = { 'config.nu' } } }",
    )
    .unwrap();
    let err = format!(
        "{:#}",
        Repo::load(Host::new(Os::Darwin, "/h", tmp.path())).unwrap_err()
    );
    assert!(
        err.contains("module `m`")
            && err.contains("plugin `nushell`")
            && err.contains("`nushell.source` entry `config.nu` is relative")
            && err.contains("no `path`"),
        "{err}"
    );
}

#[test]
fn effectful_api_is_rejected_during_load() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("m")).unwrap();
    std::fs::write(
        tmp.path().join("m/qd.lua"),
        "qd.write('/tmp/should-not-exist', 'x')\nreturn {}",
    )
    .unwrap();
    let err = format!(
        "{:#}",
        Repo::load(Host::new(Os::Darwin, "/h", tmp.path())).unwrap_err()
    );
    assert!(err.contains("setup.before or setup.after"), "{err}");
}

#[test]
fn sandbox_has_no_os_io_or_loaders() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("m")).unwrap();
    std::fs::write(
        tmp.path().join("m/qd.lua"),
        "assert(os == nil and io == nil and dofile == nil and loadfile == nil and load == nil)\n\
         assert(warn == nil, 'Lua warn shadows qd.warn')\n\
         assert(type(print) == 'function' and type(qd.warn) == 'function')\nreturn {}",
    )
    .unwrap();
    Repo::load(Host::new(Os::Darwin, "/h", tmp.path())).unwrap();
}

#[test]
fn runaway_config_hits_the_budget() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("m")).unwrap();
    std::fs::write(tmp.path().join("m/qd.lua"), "while true do end\nreturn {}").unwrap();
    let err = format!(
        "{:#}",
        Repo::load(Host::new(Os::Darwin, "/h", tmp.path())).unwrap_err()
    );
    assert!(err.contains("instruction budget"), "{err}");
}

#[test]
fn require_cannot_escape_the_repo() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("m")).unwrap();
    std::fs::write(
        tmp.path().join("m/qd.lua"),
        "require('../../etc/passwd')\nreturn {}",
    )
    .unwrap();
    let err = format!(
        "{:#}",
        Repo::load(Host::new(Os::Darwin, "/h", tmp.path())).unwrap_err()
    );
    assert!(err.contains("invalid module name"), "{err}");
}

#[test]
fn hooks_run_with_module_argument() {
    let repo = Repo::load(host(&[])).unwrap();
    let dest = tempfile::tempdir().unwrap();

    let m = repo.module("neovim").unwrap();
    let mut view = m.view();
    view.dest = Some(dest.path().to_path_buf());
    m.hooks.run_before(&view).unwrap();
    m.hooks.run_after(&view).unwrap();
    assert_eq!(
        std::fs::read_to_string(dest.path().join("generated.txt")).unwrap(),
        "hello neovim"
    );

    let m = repo.module("starship").unwrap();
    let mut view = m.view();
    view.dest = Some(dest.path().to_path_buf());
    m.hooks.run_after(&view).unwrap();
    assert_eq!(
        std::fs::read_to_string(dest.path().join("starship.nu"))
            .unwrap()
            .trim(),
        "init starship"
    );
}

#[test]
fn empty_repo_is_an_error() {
    let err = format!(
        "{:#}",
        Repo::load(Host::new(Os::Darwin, "/h", fixture("empty"))).unwrap_err()
    );
    assert!(err.contains("no qd.lua"), "{err}");
}
