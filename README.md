# qd

Dotfiles manager: one static binary, modules configured in Lua 5.5, secrets
encrypted with age, history in git.

    qd status              # what push would do
    qd push [module...]    # repo → machine, runs setup hooks on first run
    qd pull [module...]    # machine → repo
    qd pull -s "message"   # …then commit and push
    qd undo                # revert the last apply from the trash
    qd init --url ssh://…  # new machine

Every module is a directory in the dotfiles repo with a `qd.lua` that returns a
table; see `contrib/qd.d.lua` for the schema and `DESIGN.md` for how it works.
A root `qd.lua` holds shared `ignore`, `encrypt`, `include` and `dotfile`
entries.

```lua
local qd = require("qd")
return {
  path    = qd.host.windows and qd.path.local_appdata("nvim") or qd.path.config("nvim"),
  brew    = { "neovim", "ripgrep" },
  encrypt = { "**/*.p12" },
  ignore  = { "**/lazy-lock.json" },
  dotfile = { source = { "config.nu" }, env_source = { "env.nu" } },
  setup   = { version = 1, after = function(m) qd.run("fnm", "install", "--lts") end },
}
```

Files a setup hook generates into `path` belong in `ignore`; otherwise the next
`pull` copies them into the repo and the next `push` on another machine treats
them as strays.

## Machine state

`qd state path` shows the directory holding `state.toml` (tags, repo and
identity paths, per-module setup versions), `journal.jsonl`, and `trash/`.
Override it with `QD_STATE`. Removed and overwritten files go to the trash;
`qd trash prune --older 30d` cleans up.

## Keys

`master.rec` (recipients) is committed in the repo root; `master.key` is not
and lives next to it or wherever `qd state set-identity` points.

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
