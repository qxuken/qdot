use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::apply::Applier;
use crate::compile;
use crate::config::{ModuleView, RootConfig};
use crate::crypto::{Crypto, IDENTITY_FILE, RECIPIENTS_FILE};
use crate::host::Host;
use crate::journal::Journal;
use crate::packages::{self, Manager};
use crate::plan::{Direction, Plan, PlanOpts, plan};
use crate::repo::{Repo, find_root};
use crate::state::{State, state_dir};
use crate::update;
use crate::vcs::{SystemGit, Vcs};

#[derive(Parser)]
#[command(name = "qd", version, about = "dotfiles manager")]
pub struct Cli {
    /// Path to the dotfiles repository (default: QD_REPO, state file, or cwd).
    #[arg(long, global = true)]
    repo: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List modules and where they sync to.
    List,
    /// Print the resolved configuration of a module (or everything with --all).
    Show {
        module: Option<String>,
        #[arg(long, conflicts_with = "module")]
        all: bool,
        #[arg(long, value_enum, default_value_t = Format::Json)]
        format: Format,
    },
    /// Print what qd knows about this machine.
    Host,
    /// Show what push (or --pull) would do, without doing it.
    Status {
        modules: Vec<String>,
        #[arg(long)]
        pull: bool,
        #[arg(long)]
        json: bool,
    },
    /// Repo → machine. All syncable modules unless names are given.
    Push {
        modules: Vec<String>,
        /// Pull the git remote first.
        #[arg(short = 's', long)]
        sync: bool,
        #[command(flatten)]
        opts: SyncOpts,
        /// Rerun setup hooks even if already recorded.
        #[arg(long, conflicts_with = "no_setup")]
        force: bool,
        /// Skip setup hooks entirely (nothing is recorded either).
        #[arg(long)]
        no_setup: bool,
    },
    /// Machine → repo. All syncable modules unless names are given.
    Pull {
        modules: Vec<String>,
        /// Commit and push to the git remote afterwards, with this message.
        #[arg(short = 's', long, value_name = "MESSAGE")]
        sync: Option<String>,
        #[command(flatten)]
        opts: SyncOpts,
    },
    /// Regenerate ~/.dotfiles.local.nu and ~/.dotfiles-env.local.nu.
    Compile,
    /// Install or upgrade packages from every module.
    Packages {
        #[command(subcommand)]
        cmd: PackagesCmd,
    },
    /// First run on a machine: clone if needed, install packages, push everything.
    Init {
        /// Git URL to clone when the repo path does not exist yet.
        #[arg(long)]
        url: Option<String>,
        /// Where the repo lives (default: --repo, QD_REPO, state, or ~/dotfiles).
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        no_packages: bool,
    },
    /// Git operations on the repo.
    Remote {
        #[command(subcommand)]
        cmd: RemoteCmd,
    },
    /// Machine tags used by `qd.tag(...)` in configs.
    Tag {
        #[command(subcommand)]
        cmd: TagCmd,
    },
    /// Per-machine state file.
    State {
        #[command(subcommand)]
        cmd: StateCmd,
    },
    /// Revert the last apply (or a specific run id) from the journal and trash.
    Undo { run: Option<String> },
    /// Show applied operations.
    Journal {
        #[arg(long, default_value_t = 20)]
        last: usize,
    },
    /// Trash management.
    Trash {
        #[command(subcommand)]
        cmd: TrashCmd,
    },
    /// Download and install the latest version from the Forgejo package registry.
    SelfUpdate {
        #[arg(long, env = "QD_UPDATE_HOST", default_value = update::DEFAULT_HOST)]
        host: String,
        #[arg(long, env = "QD_UPDATE_OWNER", default_value = update::DEFAULT_OWNER)]
        owner: String,
        #[arg(long, default_value = update::DEFAULT_PACKAGE)]
        package: String,
        /// Token for a private package registry.
        #[arg(long, env = "QD_UPDATE_TOKEN", hide_env_values = true)]
        token: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Shell completion helper.
    #[command(name = "__complete", hide = true)]
    Complete { what: String },
}

#[derive(Args, Clone, Copy)]
struct SyncOpts {
    /// Print the plan and stop.
    #[arg(long)]
    dry_run: bool,
    /// Never remove files, only report them.
    #[arg(long)]
    no_remove: bool,
    /// Skip regenerating the Nushell entry files.
    #[arg(long)]
    no_compile: bool,
}

#[derive(Subcommand)]
enum PackagesCmd {
    Install {
        #[arg(long, value_enum)]
        manager: Option<Manager>,
        #[arg(long)]
        dry_run: bool,
    },
    Upgrade {
        #[arg(long, value_enum)]
        manager: Option<Manager>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the folded package list.
    List {
        #[arg(long, value_enum)]
        manager: Option<Manager>,
    },
}

#[derive(Subcommand)]
enum RemoteCmd {
    Pull,
    Push { message: String },
    Diff,
}

#[derive(Subcommand)]
enum TagCmd {
    List,
    Add { tag: String },
    Rm { tag: String },
}

#[derive(Subcommand)]
enum StateCmd {
    /// Print the state file.
    Show,
    /// Print the state directory.
    Path,
    /// Mark every module whose destination exists as set up at its current version.
    Adopt,
    SetRepo {
        path: PathBuf,
    },
    SetIdentity {
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum TrashCmd {
    /// Delete trash older than a duration like 30d, 12h, 90m.
    Prune {
        #[arg(long, default_value = "30d")]
        older: String,
    },
    /// Print the trash directory.
    Path,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Json,
    Toml,
}

#[derive(Serialize)]
struct ShowAll<'a> {
    root: &'a PathBuf,
    global: &'a RootConfig,
    modules: Vec<ModuleView>,
    disabled: &'a [String],
}

struct Ctx {
    state_dir: PathBuf,
    state: State,
    repo_flag: Option<PathBuf>,
}

impl Ctx {
    fn root(&self) -> Result<PathBuf> {
        find_root(
            self.repo_flag.as_deref(),
            self.state.machine.repo.as_deref(),
        )
    }

    fn host(&self) -> Result<Host> {
        Host::detect(self.root()?, self.state.machine.tags.iter().cloned())
    }

    fn repo(&self) -> Result<Repo> {
        Repo::load(self.host()?)
    }

    fn crypto(&self, root: &Path) -> Result<Crypto> {
        let identity = self
            .state
            .machine
            .identity
            .clone()
            .unwrap_or_else(|| root.join(IDENTITY_FILE));
        Crypto::load(&root.join(RECIPIENTS_FILE), Some(&identity))
    }

    fn journal(&self) -> Journal {
        Journal::new(&self.state_dir)
    }

    fn save(&self) -> Result<()> {
        self.state.save_to(&self.state_dir)
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let state_dir = state_dir()?;
    let state = State::load_from(&state_dir)?;
    let mut ctx = Ctx {
        state_dir,
        state,
        repo_flag: cli.repo,
    };

    match cli.cmd {
        Cmd::Host => println!("{}", serde_json::to_string_pretty(&ctx.host()?)?),
        Cmd::List => {
            let repo = ctx.repo()?;
            for m in &repo.modules {
                match &m.dest {
                    Some(d) => println!("{:<18} {}", m.name, d.display()),
                    None => println!("{:<18} (no path)", m.name),
                }
            }
            for name in &repo.disabled {
                println!("{name:<18} (disabled on this host)");
            }
        }
        Cmd::Show {
            module,
            all,
            format,
        } => {
            let repo = ctx.repo()?;
            if all || module.is_none() {
                let out = ShowAll {
                    root: &repo.root,
                    global: &repo.global,
                    modules: repo.modules.iter().map(|m| m.view()).collect(),
                    disabled: &repo.disabled,
                };
                print_fmt(&out, format)?;
            } else {
                print_fmt(&repo.module(module.as_deref().unwrap())?.view(), format)?;
            }
        }
        Cmd::Status {
            modules,
            pull,
            json,
        } => {
            let repo = ctx.repo()?;
            let crypto = ctx.crypto(&repo.root)?;
            let dir = if pull {
                Direction::Pull
            } else {
                Direction::Push
            };
            let plans = plan_modules(
                &repo,
                &modules,
                dir,
                &crypto,
                &ctx.state,
                PlanOpts::default(),
            )?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &plans.iter().map(|(_, p)| p).collect::<Vec<_>>()
                    )?
                );
            } else {
                print_plans(&plans, true);
            }
        }
        Cmd::Push {
            modules,
            sync,
            opts,
            force,
            no_setup,
        } => {
            let root = ctx.root()?;
            if sync {
                SystemGit.pull(&root)?;
            }
            let repo = ctx.repo()?;
            let crypto = ctx.crypto(&repo.root)?;
            let plans = plan_modules(
                &repo,
                &modules,
                Direction::Push,
                &crypto,
                &ctx.state,
                PlanOpts {
                    force,
                    no_remove: opts.no_remove,
                    no_setup,
                },
            )?;
            print_plans(&plans, false);
            if opts.dry_run {
                return Ok(());
            }
            apply_plans(&mut ctx, &repo, &crypto, &plans)?;
            if !opts.no_compile {
                let (a, b) = compile::write(&repo, &repo.host.home)?;
                println!("compiled {} and {}", a.display(), b.display());
            }
        }
        Cmd::Pull {
            modules,
            sync,
            opts,
        } => {
            let repo = ctx.repo()?;
            let crypto = ctx.crypto(&repo.root)?;
            let plans = plan_modules(
                &repo,
                &modules,
                Direction::Pull,
                &crypto,
                &ctx.state,
                PlanOpts {
                    force: false,
                    no_remove: opts.no_remove,
                    no_setup: false,
                },
            )?;
            print_plans(&plans, false);
            if opts.dry_run {
                return Ok(());
            }
            apply_plans(&mut ctx, &repo, &crypto, &plans)?;
            if !opts.no_compile {
                compile::write(&repo, &repo.host.home)?;
            }
            if let Some(message) = sync {
                if SystemGit.commit_push(&repo.root, &message)? {
                    println!("committed and pushed: {message}");
                } else {
                    println!("nothing to commit");
                }
            }
        }
        Cmd::Compile => {
            let repo = ctx.repo()?;
            let (a, b) = compile::write(&repo, &repo.host.home)?;
            println!("wrote {}\nwrote {}", a.display(), b.display());
        }
        Cmd::Packages { cmd } => {
            let repo = ctx.repo()?;
            let pick = |m: Option<Manager>| -> Result<Manager> {
                m.or_else(Manager::detect)
                    .context("neither brew nor scoop found on PATH; pass --manager")
            };
            match cmd {
                PackagesCmd::Install { manager, dry_run } => {
                    let m = pick(manager)?;
                    packages::install(m, &packages::collect(&repo, m), dry_run)?;
                }
                PackagesCmd::Upgrade { manager, dry_run } => {
                    let m = pick(manager)?;
                    packages::upgrade(m, &packages::collect(&repo, m), dry_run)?;
                }
                PackagesCmd::List { manager } => {
                    let m = pick(manager)?;
                    print_fmt(&packages::collect(&repo, m), Format::Toml)?;
                }
            }
        }
        Cmd::Init {
            url,
            path,
            no_packages,
        } => {
            let root = match path.or(ctx.repo_flag.clone()) {
                Some(p) => p,
                None => match ctx.root() {
                    Ok(r) => r,
                    Err(_) => dirs::home_dir()
                        .context("no home directory")?
                        .join("dotfiles"),
                },
            };
            if !root.exists() {
                let url = url.with_context(|| {
                    format!("{} does not exist; pass --url to clone it", root.display())
                })?;
                println!("cloning {url} into {}", root.display());
                SystemGit.clone(&url, &root)?;
            }
            let root = std::fs::canonicalize(&root)?;
            ctx.state.machine.repo = Some(root.clone());
            ctx.save()?;
            ctx.repo_flag = Some(root.clone());

            let repo = ctx.repo()?;
            if !no_packages {
                match Manager::detect() {
                    Some(m) => packages::install(m, &packages::collect(&repo, m), false)?,
                    None => println!("no package manager found, skipping packages"),
                }
            }
            let crypto = ctx.crypto(&repo.root)?;
            let plans = plan_modules(
                &repo,
                &[],
                Direction::Push,
                &crypto,
                &ctx.state,
                PlanOpts::default(),
            )?;
            print_plans(&plans, false);
            apply_plans(&mut ctx, &repo, &crypto, &plans)?;
            compile::write(&repo, &repo.host.home)?;
            println!("initialised {}", root.display());
        }
        Cmd::Remote { cmd } => {
            let root = ctx.root()?;
            match cmd {
                RemoteCmd::Pull => SystemGit.pull(&root)?,
                RemoteCmd::Push { message } => {
                    if !SystemGit.commit_push(&root, &message)? {
                        println!("nothing to commit");
                    }
                }
                RemoteCmd::Diff => print!("{}", SystemGit.diff(&root)?),
            }
        }
        Cmd::Tag { cmd } => match cmd {
            TagCmd::List => {
                for t in &ctx.state.machine.tags {
                    println!("{t}");
                }
            }
            TagCmd::Add { tag } => {
                if !ctx.state.machine.tags.contains(&tag) {
                    ctx.state.machine.tags.push(tag);
                    ctx.state.machine.tags.sort();
                    ctx.save()?;
                }
            }
            TagCmd::Rm { tag } => {
                ctx.state.machine.tags.retain(|t| t != &tag);
                ctx.save()?;
            }
        },
        Cmd::State { cmd } => match cmd {
            StateCmd::Show => print!("{}", toml::to_string_pretty(&ctx.state)?),
            StateCmd::Path => println!("{}", ctx.state_dir.display()),
            StateCmd::Adopt => {
                let repo = ctx.repo()?;
                let mut adopted = 0;
                for m in repo.syncable() {
                    if m.dest.as_ref().is_some_and(|d| d.exists()) {
                        let entry = ctx.state.modules.entry(m.name.clone()).or_default();
                        entry.setup_version = m.setup.as_ref().map(|s| s.version);
                        entry.setup_at.get_or_insert_with(crate::journal::now);
                        adopted += 1;
                        println!(
                            "adopted {:<18} setup v{}",
                            m.name,
                            entry.setup_version.unwrap_or(0)
                        );
                    }
                }
                ctx.state.machine.repo.get_or_insert(repo.root.clone());
                ctx.save()?;
                println!(
                    "{adopted} modules recorded in {}",
                    ctx.state_dir.join(crate::state::STATE_FILE).display()
                );
            }
            StateCmd::SetRepo { path } => {
                ctx.state.machine.repo = Some(std::fs::canonicalize(&path)?);
                ctx.save()?;
            }
            StateCmd::SetIdentity { path } => {
                ctx.state.machine.identity = Some(std::fs::canonicalize(&path)?);
                ctx.save()?;
            }
        },
        Cmd::Undo { run } => {
            let journal = ctx.journal();
            let run = match run {
                Some(r) => r,
                None => journal
                    .runs()?
                    .into_iter()
                    .rev()
                    .find(|r| !r.starts_with("undo-"))
                    .context("journal is empty")?,
            };
            let done = journal.undo(&run)?;
            for e in &done {
                println!("{:<8} {}", e.op, e.path.display());
            }
            println!("reverted run {run}: {} files", done.len());
        }
        Cmd::Journal { last } => {
            let entries = ctx.journal().read_all()?;
            let skip = entries.len().saturating_sub(last);
            for e in &entries[skip..] {
                println!(
                    "{}  {:<20} {:<10} {:<8} {}",
                    e.ts,
                    e.run,
                    e.module,
                    e.op,
                    e.path.display()
                );
            }
        }
        Cmd::Trash { cmd } => match cmd {
            TrashCmd::Path => println!("{}", ctx.journal().trash_root().display()),
            TrashCmd::Prune { older } => {
                let removed = ctx.journal().prune_trash(parse_duration(&older)?)?;
                println!("pruned {} trash runs", removed.len());
            }
        },
        Cmd::SelfUpdate {
            host,
            owner,
            package,
            token,
            dry_run,
        } => {
            let src = update::Source {
                host,
                owner,
                package,
                token,
            };
            match update::self_update(&src, dry_run)? {
                update::Outcome::UpToDate(v) => println!("qd {v} is up to date"),
                update::Outcome::Available { from, to } => {
                    println!("qd {to} is available (running {from})")
                }
                update::Outcome::Updated { from, to } => println!("updated qd {from} → {to}"),
            }
        }
        Cmd::Complete { what } => match what.as_str() {
            "modules" => {
                for m in ctx.repo()?.syncable() {
                    println!("{}", m.name);
                }
            }
            "all-modules" => {
                for m in &ctx.repo()?.modules {
                    println!("{}", m.name);
                }
            }
            other => bail!("unknown completion `{other}`"),
        },
    }
    Ok(())
}

fn plan_modules<'r>(
    repo: &'r Repo,
    names: &[String],
    direction: Direction,
    crypto: &Crypto,
    state: &State,
    opts: PlanOpts,
) -> Result<Vec<(&'r crate::config::Module, Plan)>> {
    let selected: Vec<&crate::config::Module> = if names.is_empty() {
        repo.syncable().collect()
    } else {
        names
            .iter()
            .map(|n| {
                let m = repo.module(n)?;
                if m.dest.is_none() {
                    bail!("module `{n}` has no `path` and cannot be synced");
                }
                Ok(m)
            })
            .collect::<Result<_>>()?
    };
    selected
        .into_iter()
        .map(|m| Ok((m, plan(m, direction, crypto, state, opts)?)))
        .collect()
}

fn apply_plans(
    ctx: &mut Ctx,
    repo: &Repo,
    crypto: &Crypto,
    plans: &[(&crate::config::Module, Plan)],
) -> Result<()> {
    let journal = ctx.journal();
    let state_dir = ctx.state_dir.clone();
    let mut applier = Applier::new(crypto, &journal, &mut ctx.state, &state_dir);
    let mut total = 0;
    for (m, p) in plans {
        if p.is_empty() {
            continue;
        }
        let report = applier.apply(m, p)?;
        total += report.applied;
    }
    if total > 0 || plans.iter().any(|(_, p)| p.first_run) {
        println!("applied {total} operations (run {})", applier.run);
    } else {
        println!("nothing to do");
    }
    let _ = repo;
    Ok(())
}

fn print_plans(plans: &[(&crate::config::Module, Plan)], verbose_empty: bool) {
    let color = std::io::stdout().is_terminal();
    for (m, p) in plans {
        if p.is_empty() && p.skipped_removes.is_empty() {
            if verbose_empty {
                println!("{:<18} up to date", m.name);
            }
            continue;
        }
        let arrow = match p.direction {
            Direction::Push => "→",
            Direction::Pull => "←",
        };
        let setup = match (&p.first_run, &p.first_run_reason, &m.setup) {
            (true, Some(reason), Some(s)) => format!("   [setup v{}: {reason}]", s.version),
            _ => String::new(),
        };
        println!("{} {arrow} {}{setup}", m.name, p.dest.display());
        for op in &p.ops {
            let target = display_target(op.target(), &p.dest, &m.src);
            if op.is_remove() && color {
                println!("  \x1b[31m{:<8} {target}\x1b[0m", op.verb());
            } else {
                println!("  {:<8} {target}", op.verb());
            }
        }
        for path in &p.skipped_removes {
            println!(
                "  {:<8} {} (skipped, --no-remove)",
                "keep",
                display_target(path, &p.dest, &m.src)
            );
        }
    }
}

fn display_target(path: &Path, dest: &Path, src: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(dest) {
        return rel.display().to_string();
    }
    if let Ok(rel) = path.strip_prefix(src) {
        return format!("repo:{}", rel.display());
    }
    path.display().to_string()
}

fn print_fmt<T: Serialize>(value: &T, format: Format) -> Result<()> {
    let text = match format {
        Format::Json => serde_json::to_string_pretty(value)?,
        Format::Toml => toml::to_string_pretty(value)?,
    };
    println!("{text}");
    Ok(())
}

fn parse_duration(s: &str) -> Result<jiff::SignedDuration> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = num.parse().with_context(|| format!("bad duration `{s}`"))?;
    let secs = match unit {
        "d" | "" => n * 86_400,
        "h" => n * 3_600,
        "m" => n * 60,
        "s" => n,
        _ => bail!("bad duration unit in `{s}` (use d, h, m, s)"),
    };
    Ok(jiff::SignedDuration::from_secs(secs))
}
