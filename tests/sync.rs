use std::path::{Path, PathBuf};

use qd::apply::Applier;
use qd::crypto::Crypto;
use qd::host::{Host, Os};
use qd::journal::Journal;
use qd::plan::{Direction, Op, Plan, PlanOpts, plan};
use qd::repo::Repo;
use qd::state::State;

struct Sandbox {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    state_dir: PathBuf,
    crypto: Crypto,
    state: State,
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

const MODULE: &str = r#"
local qd = require("qd")
return {
  path = qd.path.home(".config/app"),
  encrypt = { "**/*.secret" },
  ignore = { "**/gen.txt" },
  files = { { src = "rc", dest = qd.path.home(".apprc") } },
  setup = {
    version = VERSION,
    after = function(m) qd.write(qd.path.join(m.dest, "gen.txt"), "gen") end,
  },
}
"#;

impl Sandbox {
    fn new() -> Sandbox {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let home = tmp.path().join("home");
        let state_dir = tmp.path().join("state");
        std::fs::create_dir_all(&home).unwrap();

        let (identity, recipient) = Crypto::generate();
        write(&repo.join("master.key"), &identity);
        write(&repo.join("master.rec"), &recipient);
        let crypto =
            Crypto::load(&repo.join("master.rec"), Some(&repo.join("master.key"))).unwrap();

        write(
            &repo.join("qd.lua"),
            r#"local qd = require("qd")
return { include = { qd.path.dotfiles(".editorconfig") }, ignore = { "**/.editorconfig", "**/.DS_Store" } }"#,
        );
        write(&repo.join(".editorconfig"), "root = true\n");
        write(&repo.join("app/qd.lua"), &MODULE.replace("VERSION", "1"));
        write(&repo.join("app/a.txt"), "a1");
        write(&repo.join("app/sub/b.txt"), "b1");
        write(&repo.join("app/rc"), "rc1");
        let cipher = crypto.encrypt(b"s1").unwrap();
        std::fs::write(repo.join("app/k.secret.age"), cipher).unwrap();

        Sandbox {
            _tmp: tmp,
            repo,
            home,
            state_dir,
            crypto,
            state: State::default(),
        }
    }

    fn set_version(&self, v: u32) {
        write(
            &self.repo.join("app/qd.lua"),
            &MODULE.replace("VERSION", &v.to_string()),
        );
    }

    fn load(&self) -> Repo {
        Repo::load(Host::new(Os::Darwin, &self.home, &self.repo)).unwrap()
    }

    fn plan(&self, repo: &Repo, dir: Direction, opts: PlanOpts) -> Plan {
        plan(
            repo.module("app").unwrap(),
            dir,
            &self.crypto,
            &self.state,
            opts,
        )
        .unwrap()
    }

    fn apply(&mut self, repo: &Repo, p: &Plan) -> String {
        let journal = Journal::new(&self.state_dir);
        let mut a = Applier::new(&self.crypto, &journal, &mut self.state, &self.state_dir);
        a.apply(repo.module("app").unwrap(), p).unwrap().run
    }

    fn dest(&self) -> PathBuf {
        self.home.join(".config/app")
    }

    fn journal(&self) -> Journal {
        Journal::new(&self.state_dir)
    }
}

fn verbs(p: &Plan) -> Vec<(String, String)> {
    p.ops
        .iter()
        .map(|op| {
            (
                op.verb().to_owned(),
                op.target()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .collect()
}

#[test]
fn push_creates_dest_runs_setup_and_converges() {
    let mut sb = Sandbox::new();
    let repo = sb.load();

    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    assert!(p.first_run);
    assert_eq!(
        p.first_run_reason.as_deref(),
        Some("destination does not exist")
    );
    let mut v = verbs(&p);
    v.sort();
    assert_eq!(
        v,
        [
            ("copy", ".apprc"),
            ("copy", ".editorconfig"),
            ("copy", "a.txt"),
            ("copy", "b.txt"),
            ("decrypt", "k.secret"),
        ]
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
    );

    sb.apply(&repo, &p);
    let d = sb.dest();
    assert_eq!(read(&d.join("a.txt")), "a1");
    assert_eq!(read(&d.join("sub/b.txt")), "b1");
    assert_eq!(read(&d.join("k.secret")), "s1");
    assert_eq!(read(&d.join(".editorconfig")), "root = true\n");
    assert_eq!(read(&sb.home.join(".apprc")), "rc1");
    assert_eq!(read(&d.join("gen.txt")), "gen", "setup.after ran");

    let ms = &sb.state.modules["app"];
    assert_eq!(ms.setup_version, Some(1));
    assert!(ms.setup_at.is_some() && ms.last_push.is_some());
    assert!(sb.state_dir.join("state.toml").exists());

    let again = sb.plan(&repo, Direction::Push, PlanOpts::default());
    assert!(again.is_empty(), "second push plans nothing: {again:?}");

    let entries = sb.journal().read_all().unwrap();
    assert_eq!(entries.len(), 5);
    assert!(
        !d.join("rc").exists(),
        "pair sources are not mirrored into the module dir"
    );
    assert!(entries.iter().all(|e| e.created && e.trash.is_none()));
}

#[test]
fn pull_round_trip_with_encryption() {
    let mut sb = Sandbox::new();
    let repo = sb.load();
    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    sb.apply(&repo, &p);

    let d = sb.dest();
    write(&d.join("a.txt"), "a2");
    write(&d.join("new.txt"), "n1");
    write(&d.join("k.secret"), "s2");
    write(&sb.home.join(".apprc"), "rc2");

    let p = sb.plan(&repo, Direction::Pull, PlanOpts::default());
    let mut v = verbs(&p);
    v.sort();
    assert_eq!(
        v,
        [
            ("copy", "a.txt"),
            ("copy", "new.txt"),
            ("copy", "rc"),
            ("encrypt", "k.secret.age")
        ]
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
    );
    assert!(!p.first_run, "pull never runs setup");

    sb.apply(&repo, &p);
    assert_eq!(read(&sb.repo.join("app/a.txt")), "a2");
    assert_eq!(read(&sb.repo.join("app/new.txt")), "n1");
    assert_eq!(read(&sb.repo.join("app/rc")), "rc2");
    let cipher = std::fs::read(sb.repo.join("app/k.secret.age")).unwrap();
    assert_eq!(sb.crypto.decrypt(&cipher).unwrap(), b"s2");
    assert!(!sb.repo.join("app/k.secret").exists());
    assert!(sb.state.modules["app"].last_pull.is_some());

    assert!(
        sb.plan(&repo, Direction::Push, PlanOpts::default())
            .is_empty()
    );
    assert!(
        sb.plan(&repo, Direction::Pull, PlanOpts::default())
            .is_empty()
    );
}

#[test]
fn pull_moves_a_file_into_encryption_when_globs_change() {
    let mut sb = Sandbox::new();
    write(&sb.repo.join("app/x.secret"), "plain-in-repo");
    let repo = sb.load();
    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    sb.apply(&repo, &p);
    assert_eq!(read(&sb.dest().join("x.secret")), "plain-in-repo");

    let p = sb.plan(&repo, Direction::Pull, PlanOpts::default());
    assert_eq!(
        p.ops,
        [
            Op::Encrypt {
                from: sb.dest().join("x.secret"),
                to: sb.repo.join("app/x.secret.age")
            },
            Op::Remove {
                path: sb.repo.join("app/x.secret")
            },
        ]
    );
    sb.apply(&repo, &p);
    assert!(!sb.repo.join("app/x.secret").exists());
    assert!(sb.repo.join("app/x.secret.age").exists());
    assert!(
        sb.plan(&repo, Direction::Push, PlanOpts::default())
            .is_empty()
    );
}

#[test]
fn removes_are_visible_skippable_trashed_and_undoable() {
    let mut sb = Sandbox::new();
    let repo = sb.load();
    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    sb.apply(&repo, &p);

    std::fs::remove_file(sb.repo.join("app/sub/b.txt")).unwrap();
    let gone = sb.dest().join("sub/b.txt");

    let p = sb.plan(
        &repo,
        Direction::Push,
        PlanOpts {
            no_remove: true,
            ..Default::default()
        },
    );
    assert!(p.ops.is_empty());
    assert_eq!(p.skipped_removes, std::slice::from_ref(&gone));

    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    assert_eq!(p.ops, [Op::Remove { path: gone.clone() }]);
    assert!(p.ops[0].is_remove());

    let run = sb.apply(&repo, &p);
    assert!(!gone.exists());
    let entries = sb.journal().read_all().unwrap();
    let last = entries.last().unwrap();
    assert_eq!(
        (last.run.as_str(), last.op.as_str()),
        (run.as_str(), "remove")
    );
    let trashed = last.trash.clone().unwrap();
    assert!(trashed.starts_with(sb.state_dir.join("trash")));
    assert_eq!(read(&trashed), "b1");

    let undone = sb.journal().undo(&run).unwrap();
    assert_eq!(undone.len(), 1);
    assert_eq!(read(&gone), "b1");
    assert_eq!(sb.journal().runs().unwrap().len(), 3, "push, remove, undo");

    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    assert_eq!(
        p.ops,
        [Op::Remove { path: gone }],
        "undo restored the file, so the remove is planned again"
    );
}

#[test]
fn undo_restores_overwritten_content_and_deletes_created_files() {
    let mut sb = Sandbox::new();
    let repo = sb.load();
    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    let first = sb.apply(&repo, &p);

    write(&sb.repo.join("app/a.txt"), "a2");
    write(&sb.repo.join("app/c.txt"), "c1");
    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    assert!(!p.first_run);
    let run = sb.apply(&repo, &p);
    assert_eq!(read(&sb.dest().join("a.txt")), "a2");
    assert_eq!(read(&sb.dest().join("c.txt")), "c1");

    sb.journal().undo(&run).unwrap();
    assert_eq!(read(&sb.dest().join("a.txt")), "a1");
    assert!(!sb.dest().join("c.txt").exists());

    sb.journal().undo(&first).unwrap();
    assert!(!sb.dest().join("a.txt").exists());
    assert!(!sb.home.join(".apprc").exists());
}

#[test]
fn setup_reruns_on_version_bump_or_force_only() {
    let mut sb = Sandbox::new();
    let repo = sb.load();
    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    sb.apply(&repo, &p);
    std::fs::remove_file(sb.dest().join("gen.txt")).unwrap();

    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    assert!(
        p.is_empty(),
        "generated file is ignored and setup already recorded: {p:?}"
    );

    let p = sb.plan(
        &repo,
        Direction::Push,
        PlanOpts {
            force: true,
            no_remove: true,
            ..Default::default()
        },
    );
    assert_eq!(p.first_run_reason.as_deref(), Some("--force"));
    sb.apply(&repo, &p);
    assert!(sb.dest().join("gen.txt").exists());

    sb.set_version(2);
    let repo = sb.load();
    let p = sb.plan(
        &repo,
        Direction::Push,
        PlanOpts {
            no_remove: true,
            ..Default::default()
        },
    );
    assert!(p.first_run);
    assert_eq!(p.first_run_reason.as_deref(), Some("setup version 1 → 2"));
    sb.apply(&repo, &p);
    assert_eq!(sb.state.modules["app"].setup_version, Some(2));
    assert!(
        !sb.plan(
            &repo,
            Direction::Push,
            PlanOpts {
                no_remove: true,
                ..Default::default()
            }
        )
        .first_run
    );
}

#[test]
fn no_setup_skips_hooks_and_records_nothing() {
    let mut sb = Sandbox::new();
    let repo = sb.load();
    let p = sb.plan(
        &repo,
        Direction::Push,
        PlanOpts {
            no_setup: true,
            ..Default::default()
        },
    );
    assert!(!p.first_run && !p.ops.is_empty());
    sb.apply(&repo, &p);
    assert!(!sb.dest().join("gen.txt").exists());
    assert_eq!(sb.state.modules["app"].setup_version, None);
    let p = sb.plan(&repo, Direction::Push, PlanOpts::default());
    assert_eq!(
        p.first_run_reason.as_deref(),
        Some("no setup recorded on this machine")
    );
}

#[test]
fn missing_identity_fails_only_when_an_encrypted_file_needs_comparing() {
    let sb = Sandbox::new();
    let no_keys = Crypto::load(&sb.repo.join("nope"), None).unwrap();
    let repo = sb.load();
    let m = repo.module("app").unwrap();
    let p = plan(m, Direction::Push, &no_keys, &sb.state, PlanOpts::default()).unwrap();
    assert!(
        p.ops.iter().any(|o| matches!(o, Op::Decrypt { .. })),
        "planning a first push needs no key"
    );

    let journal = Journal::new(&sb.state_dir);
    let mut state = State::default();
    let mut a = Applier::new(&no_keys, &journal, &mut state, &sb.state_dir);
    let err = format!("{:#}", a.apply(m, &p).unwrap_err());
    assert!(err.contains("master.key"), "{err}");
}

#[test]
fn trash_prune_removes_old_runs_only() {
    let sb = Sandbox::new();
    let j = sb.journal();
    let old = j.trash_root().join("1000-1");
    let new = j
        .trash_root()
        .join(format!("{}-1", jiff::Timestamp::now().as_millisecond()));
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    let removed = j.prune_trash(jiff::SignedDuration::from_hours(24)).unwrap();
    assert_eq!(removed, std::slice::from_ref(&old));
    assert!(!old.exists() && new.exists());
}
