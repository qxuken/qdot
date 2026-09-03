# qd — dotfiles manager design

Replacement for `dotfiles.nu`: a single static binary, Rust, Lua-configured, git-backed, age-encrypted.

## Decisions

| Area | Choice | Why |
|---|---|---|
| Language | Rust | mlua vendors Lua 5.5; age crate is format-compatible with the age CLI |
| Config | one `qd.lua` per module + root `qd.lua` | replaces `qd-config.yml` and `qd-init.nu`; host merging and `%tokens%` become plain Lua |
| Lua engine | `mlua` with `lua55` + `vendored` + `serialize` features | no system Lua, cross-compiles with `cc` |
| Encryption | `age` crate, X25519, `master.rec` committed / `master.key` ignored | unchanged from today; SSH-key identities via `age::ssh` are an optional later step |
| VCS | git on Forgejo, behind a `Vcs` trait; first impl shells out to system git, `git2` backend later | avoids vendored libgit2/libssh2/openssl cross-build pain up front |
| Hosting/CI | Forgejo Actions on one Linux runner, cross-compile matrix, upload to Forgejo Releases | `qd self-update` reads the releases API |
| Machine state | `state.toml` in the platform state dir, plus `journal.jsonl` and a trash dir | replaces dest-exists first-run detection and `DOTFILES_TAGS` env |
| Sync semantics | content-hash compare, plan/apply split, atomic writes, removes go to trash | fixes mtime clobbering from fresh clones; makes `status`/`--dry-run` free |
| Caching | none | thirteen small modules; add a `[cache]` table later if ever needed |

## Repos

- `qd` — the tool, CI, releases.
- `dotfiles` — data. Migrate from fossil to git (`fossil export --git` → `git fast-import`). Optional Forgejo push mirror to GitHub.

## Module file

```lua
-- neovim/qd.lua
local qd = require("qd")
local pkgs = { "git", "fzf", "ripgrep", "fd", "cmake", "llvm", "fnm", "neovim" }

return {
  enabled = true,                                  -- default true; e.g. `not qd.tag("wsl")`
  path    = qd.host.windows and qd.path.local_appdata("nvim") or qd.path.config("nvim"),
  brew    = qd.host.ubuntu and qd.list(pkgs, "xclip", "xsel") or pkgs,
  scoop   = pkgs,                                  -- entries: string or { name=, bucket= } / { name=, tap= }
  files   = {                                      -- extra pairs outside `path`
    -- src is relative to this module's directory in the repo (may be stored as src.age);
    -- a pair source is owned by the pair and is not mirrored into `path`
    { src = "gitconfig", dest = qd.path.home(".gitconfig"), enabled = not qd.tag("work") },
  },
  include = { qd.path.dotfiles(".editorconfig") }, -- copied into dest on push only
  ignore  = { "**/history.txt" },                  -- globs relative to dest; list files your setup hook generates here
  encrypt = { "**/*.p12" },                        -- globs relative to dest; stored as `<name>.age`
  dotfile = {
    include    = { "vpn.nu" },                     -- `use`
    source     = { "config.nu" },                  -- `source`
    env_include = {},
    env_source = { "env.nu" },
  },
  setup = {
    version = 1,                                   -- bump to rerun on every machine
    before  = function(m) end,                     -- runs before first apply
    after   = function(m) qd.run("fnm", "install", "--lts") end,
  },
}
```

Root `qd.lua` returns the same shape; its `ignore`, `encrypt`, `include`, and `dotfile` apply to every module. Modules can `qd.require("lib")` for helpers, restricted to the repo directory.

Hook argument `m`: `{ name, src, dest, config = <resolved table> }`.

## `qd` Lua API

- `qd.host` — booleans `darwin`, `ubuntu`, `windows`, `posix`, `wsl`; `qd.host.name`.
- `qd.tag(name)` — machine tag from `state.toml`.
- `qd.path.home(...)`, `.config(...)`, `.cache(...)` (`~/.dotfiles-cache`), `.app_support(...)`, `.appdata(...)`, `.local_appdata(...)`, `.dotfiles(...)`, `.join(...)`.
- `qd.list(base, ...)` — append; replaces `%root%`.
- `qd.env(name)`, `qd.exists(path)`.
- Hook phase only: `qd.run(cmd, ...)` (streams, errors on non-zero), `qd.exec(cmd, ...)` (captures stdout), `qd.write(path, content)`.

Sandbox: fresh `Lua` per file; only `base`, `string`, `table`, `math` opened; no `os`/`io`; instruction budget on load. Hook-phase functions raise outside hooks (phase flag on the host). Ship `qd.d.lua` type stubs for lua-language-server.

## Data flow

```
CLI args + env ──► Host { os, tags, base paths }
repo tree      ──► Discover */qd.lua + root qd.lua
Host + files   ──► Lua load (pure) ──► Module { src, dest, globs, files, include, packages, dotfile, hooks }
Module         ──► Planner: scan src, scan dest, diff by content hash ──► Plan { first_run, ops }
Plan           ──► status / --dry-run (print)   |   Apply (fs + age) ──► setup.before / setup.after
Modules        ──► Packages (brew/scoop)   |   Dotfile compiler (~/.dotfiles.local.nu, ~/.dotfiles-env.local.nu)
```

Core types:

```rust
struct Host { os: Os, tags: BTreeSet<String>, home: PathBuf, config: PathBuf, cache: PathBuf, /* ... */ }

struct Module {
    name: String, src: PathBuf, dest: Option<PathBuf>,
    ignore: GlobSet, encrypt: GlobSet,
    files: Vec<FilePair>, include: Vec<PathBuf>,
    packages: Packages, dotfile: DotfileEntries,
    hooks: Hooks,            // keeps the Lua state + owned Function handles
}

enum Op {
    Copy    { from: PathBuf, to: PathBuf },
    Encrypt { from: PathBuf, to: PathBuf },
    Decrypt { from: PathBuf, to: PathBuf },
    Remove  { path: PathBuf },
}
struct Plan { module: String, first_run: bool, ops: Vec<Op> }
```

Planner rules:
- Scan each side into `BTreeMap<RelPath, Entry { abs, encrypted }>` where `encrypted` means the bytes on disk are age ciphertext (only ever true on the repo side, from the `.age` suffix). Whether a dest file *should* be encrypted comes from the `encrypt` globs at plan time.
- Compare plaintext bytes; encrypted entries are decrypted in memory (ciphertext is randomized).
- Files named by a `files[].src` pair are removed from both trees; the pair op reads the repo file directly.
- On pull, a file whose encryption state changed (globs edited) gets the new form written and the old form removed.
- Direction picks the authoritative side: push = src → dest, pull = dest → src.
- Extra `files` pairs and `include` entries add ops the same way.
- Removes are ordered last and are the only data-losing op: shown distinctly in `status`, disabled by `--no-remove`, and moved to trash instead of unlinked.

Apply rules:
- Temp file in target dir + rename.
- Recipients/identities loaded once per run.
- `setup.before` runs before ops, `setup.after` after, only when `first_run`.

First-run rule (`first_run = true` when any holds):
- dest path missing
- no `[modules.<name>]` entry in state
- stored `setup_version` < module `setup.version`
- `--force`

## Machine state

Location: platform state dir (`~/.local/state/qd` on Linux, Application Support on macOS, `%LOCALAPPDATA%\qd` on Windows), override with `QD_STATE`.

```toml
[machine]
tags = ["work"]
repo = "/Users/qxuken/dotfiles"
identity = "/Users/qxuken/dotfiles/master.key"

[modules.starship]
setup_version = 2
setup_at = 2026-09-04T10:12:00Z
last_push = 2026-09-04T10:12:00Z
```

- `journal.jsonl` — one line per applied op and hook run.
- `trash/<module>/<timestamp>/<relpath>` — removed files; `qd undo` restores the last apply, `qd trash prune --older 30d`.
- `qd state adopt` — one-off migration: mark every module whose dest exists as initialized at its current version.

## Commands

| Command | Notes |
|---|---|
| `qd status [modules..] [--pull] [--json]` | print the plan, removes in red, `up to date` otherwise |
| `qd push [modules..] [-s] [--dry-run] [--no-remove] [--force] [--no-compile]` | all syncable modules when none named; `-s` pulls the remote first; recompiles after |
| `qd pull [modules..] [-s <msg>] [--dry-run] [--no-remove] [--no-compile]` | `-s` commits and pushes after |
| `qd init [--url] [--path] [--no-packages]` | clone if needed, record repo in state, packages, push, compile |
| `qd show [module] [--all] [--format json\|toml]` | resolved config, for verifying against old semantics |
| `qd list`, `qd host` | modules and destinations; detected host |
| `qd packages install\|upgrade\|list [--manager] [--dry-run]` | brew or scoop, auto-detected |
| `qd compile` | regenerate the two Nushell files |
| `qd remote pull\|push <msg>\|diff` | via `Vcs` trait (system git) |
| `qd tag list\|add\|rm` | machine tags in state |
| `qd state show\|path\|adopt\|set-repo\|set-identity` | state file |
| `qd undo [run]`, `qd journal [--last]`, `qd trash prune [--older 30d]\|path` | journal/trash |
| `qd self-update [--api] [--dry-run]` | Forgejo releases API, SHA-256 verified |
| `qd __complete modules\|all-modules` | for `contrib/qd.nu` |

Git wraps the pipeline: `push -s` fetches and fast-forwards before discovery; `pull -s` commits and pushes after apply.

## Crates

clap (derive, env), mlua (`lua55`, `vendored`, `serialize`, `error-send`), serde + toml + serde_json, globset + walkdir, age, sha2 (release checksums), jiff, dirs, ureq + self-replace (self-update), anyhow. `git2` only when the libgit backend is added.

## Release pipeline

`.forgejo/workflows/ci.yml` follows the proven kui workflow: a `check` job (fmt, clippy, test) on every push, and on a `v*` tag a five-target build matrix on the single Linux docker runner using `node:24-bookworm` containers with rustup inside, cargo-zigbuild plus the ziglang wheel for Linux (musl, static), Windows (gnu) and macOS (with the cached MacOSX SDK for framework stubs). A `publish` job checks the tag against `Cargo.toml`, writes `checksums.sha256`, and uploads everything to the Forgejo **generic package registry** as package `qd` version `X.Y.Z` using the `PACKAGES_TOKEN` secret (write:packages). `qd self-update` lists versions through `/api/v1/packages/{owner}?type=generic&q=qd`, downloads `qd-<os>-<arch>[.exe]` from `/api/packages/{owner}/generic/qd/{version}/`, verifies the checksum, and swaps the binary. Defaults: host `https://drydock9.qxuken.dev`, owner `qxuken`; override with `--host/--owner` or `QD_UPDATE_HOST/QD_UPDATE_OWNER`. Unverified until the first tag is pushed.

## Milestones

1. Migrate `dotfiles` fossil → git on Forgejo. Independent of the tool.
2. Scaffold `qd`: Host, Lua host + `qd` API, discovery, `show`, read-only `status`. Hand-convert the 13 YAML configs and 6 init scripts; verify with `show`.
3. Planner + apply + age: `push`, `pull`, `-all`, `--dry-run`, `--no-remove`, trash/journal.
4. State file, setup versioning, `state adopt`, tags.
5. Packages, `compile`, `init`.
6. `Vcs` trait with system git; `remote *`; `-s` flags.
7. Forgejo Actions release matrix + `self-update`.
8. Swap Nushell aliases and completer, delete `dotfiles.nu` and `.fossil-settings`.

## Open

- Keep the GitHub mirror (Forgejo push mirror) or drop it.
- Move to SSH-key age identities now or after migration.
- `git2` backend: needed at all, or is system git enough long-term.
