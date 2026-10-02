# qd

Dotfiles manager: one static binary, modules configured in Lua 5.5, secrets
encrypted with age, history in git.

    qd status              # what push would do
    qd push [module...]    # repo → machine, runs setup hooks on first run
    qd pull [module...]    # machine → repo
    qd pull -s "message"   # …then commit and push
    qd undo                # revert the last apply from the trash
    qd init --url ssh://…  # new machine
    qd add NAME PATH       # start keeping a directory as module NAME

Every module is a directory in the dotfiles repo with a `qd.lua` that returns a
table; see `contrib/qd.d.lua` for the schema and `DESIGN.md` for how it works.
A root `qd.lua` holds shared `ignore`, `encrypt` and `include` entries, plus
whatever the plugins read from it.

```lua
local qd = require("qd")
return {
  path    = qd.host.windows and qd.path.local_appdata("nvim") or qd.path.config("nvim"),
  brew    = { "neovim", "ripgrep" },
  encrypt = { "**/*.p12" },
  ignore  = { "**/lazy-lock.json" },
  nushell = { source = { "config.nu" }, env_source = { "env.nu" } },
  setup   = { version = 1, after = function(m) qd.run("fnm", "install", "--lts") end },
}
```

`brew`, `scoop` and `nushell` are not core fields: they belong to plugins. The
core syncs files and runs setup hooks; a plugin owns one key in every `qd.lua`
and turns it into files to write (`compile`) or commands to run (`packages`),
both pure, so `--dry-run` and `undo` cover them. The three built-ins load
unless the root file declares its own list:

```lua
-- root qd.lua
return {
  plugins = { require("qd.nushell"), require("qd.brew"), require("plugins.mine") },
}
```

Plugins get `qd.fail` for errors, `qd.check` with `qd.schema` for validating
their key, and
`qd.warn` / `qd.debug` for output. All plugin and config output goes to
stderr, `print` included, so `qd show --format json` stays parseable. See the
Plugins section of `DESIGN.md` for the contract and `contrib/apt.lua` for a
worked example.

Files a setup hook generates into `path` belong in `ignore`; otherwise the next
`pull` copies them into the repo and the next `push` on another machine treats
them as strays.

## Machine state

`qd state path` shows the directory holding `state.toml` (tags, repo and
identity paths, per-module setup versions), `journal.jsonl`, and `trash/`.
Override it with `QD_STATE`. Removed and overwritten files go to the trash;
`qd trash prune --older 30d` cleans up.

Tags gate `enabled` in a config. `qd tag add <name>` records one in `state.toml`,
which is what you want on a machine you own; `QD_TAGS=a,b` adds tags for one
command, which is handy for trying a config out (`QD_TAGS=work qd show
version-control`). `DOTFILES_TAGS` is read too, for the Nushell tool this
replaces. Detected facts are not tags — a config asks `qd.host.wsl` or
`qd.host.ubuntu` for those.

## Keys

`master.rec` (recipients) is committed in the repo root; `master.key` is not
and lives next to it or wherever `qd state set-identity` points.

## As a library

The crate is `qdot` (`qd` is another crate's name on crates.io); the
library and the binary are `qd`. Without its default `cli` feature it is
the library alone — no clap, ureq or self-replace — and `qd::Session`
does what the binary does, returning data and printing nothing:

```toml
qdot = { version = "0.2", default-features = false, registry = "drydock9" }
```

```rust
let mut s = qd::Session::open(None)?;            // QD_STATE, the recorded repo
let plans = s.status(&[], qd::plan::Direction::Push)?;
let done = s.sync(&["helix".into()], qd::plan::Direction::Pull, qd::SyncOpts::default())?;
s.add_module("kawoosh", &home.join(".config/kawoosh"), &["fonts/**".into()])?;
```

A program linking it shares `state.toml`, the journal and the trash with
the `qd` on the PATH, so it should hold to `qd::VERSION` matching that
binary's, and run the binary when they differ. kawoosh's dotfiles pane
does.

## Build

    cargo build --release

Releases are cut by pushing a `vX.Y.Z` tag matching `Cargo.toml`;
`.forgejo/workflows/ci.yml` cross-compiles every target with cargo-zigbuild on
the Linux runner and uploads the binaries to the Forgejo generic package
registry, where `qd self-update` finds them.

## Installing

`contrib/install.nu` picks the right binary for the machine, verifies its
SHA-256 against the published checksums, and installs it into `~/.local/bin`
(or `%LOCALAPPDATA%\qd\bin`):

    nu contrib/install.nu
    nu contrib/install.nu --version 0.1.0 --dest ~/bin
    nu contrib/install.nu --init-url ssh://git@drydock9-port1.qxuken.dev/qxuken/dotfiles.git

From 0.1.1 onward it is published beside the binaries, so a machine with
nushell but no clone can fetch it first (0.1.0 shipped binaries only):

    curl -fLO https://drydock9.qxuken.dev/api/packages/qxuken/generic/qd/<version>/install.nu

Without nushell, download the asset for the platform directly
(`qd-linux-x86_64`, `qd-macos-aarch64`, `qd-windows-x86_64.exe`, …) from
`.../generic/qd/<version>/` and `chmod +x` it.
