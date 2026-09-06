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
| Plugins | Lua tables declared by the root `qd.lua`; the Nushell compiler and brew/scoop ship as built-ins | keeps the core a pure sync pipeline; anything shell- or package-manager-specific is data folded by a plugin |
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
  brew    = qd.host.ubuntu and qd.list(pkgs, "xclip", "xsel") or pkgs, -- plugin key (built-in `brew`)
  scoop   = pkgs,                                  -- plugin key; entries: string or { name=, bucket= } / { name=, tap= }
  files   = {                                      -- extra pairs outside `path`
    -- src is relative to this module's directory in the repo (may be stored as src.age);
    -- a pair source is owned by the pair and is not mirrored into `path`
    { src = "gitconfig", dest = qd.path.home(".gitconfig"), enabled = not qd.tag("work") },
  },
  include = { qd.path.dotfiles(".editorconfig") }, -- copied into dest on push only
  ignore  = { "**/history.txt" },                  -- globs relative to dest; list files your setup hook generates here
  encrypt = { "**/*.p12" },                        -- globs relative to dest; stored as `<name>.age`
  nushell = {                                      -- plugin key (built-in `nushell`)
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

Core fields are `enabled`, `path`, `files`, `include`, `ignore`, `encrypt`, `setup`. Every other key must be the name of a loaded plugin, which validates and resolves it; a typo is an error naming the module and the loaded plugins.

Root `qd.lua` returns the same shape; its `ignore`, `encrypt`, `include` and plugin keys apply to every module, and it may declare `plugins`. Modules can `require("lib")` for helpers, restricted to the repo directory.

Hook argument `m`: the module's `qd show` view (`name`, `src`, `dest`, core fields, plugin keys).

## Plugins

A plugin is a Lua table. The root file lists them; with no `plugins` key the built-ins load in this order: `qd.nushell`, `qd.brew`, `qd.scoop`. Declaring the list replaces that set, so built-ins are opted back in with `require("qd.<name>")`.

```lua
-- root qd.lua
return {
  plugins = { require("qd.nushell"), require("qd.brew"), require("plugins.aliases") },
}
```

```lua
-- plugins/aliases.lua: everything optional except `name`, everything pure
local qd = require("qd")
return {
  name = "aliases",                                 -- owns the `aliases` key in every qd.lua
  available = function() return qd.which("brew") ~= nil end, -- gates package detection
  resolve = function(m, value)                      -- load time; m = { name, src, dest, root }
    if m.root then qd.fail("`aliases` belongs in modules") end
    return qd.check(value, qd.schema.list(qd.schema.string()), "aliases")
  end,
  compile = function(ctx)                           -- ctx = { root, global, modules }
    return { { path = qd.path.home(".aliases"), content = "..." } }
  end,
  packages = {
    list    = function(ctx) return { ... } end,     -- printed by `qd packages list`
    install = function(ctx) return { { "brew", "install", "x" } } end, -- argv lists, run by the core
    upgrade = function(ctx) return { ... } end,
  },
}
```

Plugins never write or execute anything: `compile` returns file contents and the core diffs, writes atomically, trashes the previous version and journals under the same run as the apply, so `status`/`--dry-run` show pending compile output and `undo` reverts it. `packages.*` return commands and the core prints and runs them, so `--dry-run` is free there too. `qd packages` picks the first plugin with a `packages` section whose `available()` holds, or `--manager <name>`.

`contrib/apt.lua` is a complete worked example of a third-party package plugin. The built-in `nushell` plugin resolves relative entries against the module's `path`, folds every module's `nushell` table plus the root's into `~/.dotfiles.local.nu` and `~/.dotfiles-env.local.nu`, unique and reverse sorted so `use` lines precede `source` lines. `brew` and `scoop` fold the package lists (first occurrence wins, taps/buckets collected in first-seen order).

## `qd` Lua API

- `qd.host` — booleans `darwin`, `ubuntu`, `windows`, `posix`, `wsl`; `qd.host.name`.
- `qd.tag(name)` — machine tag from `state.toml`.
- `qd.path.home(...)`, `.config(...)`, `.cache(...)` (`~/.dotfiles-cache`), `.app_support(...)`, `.appdata(...)`, `.local_appdata(...)`, `.dotfiles(...)`, `.join(...)`.
- `qd.list(base, ...)` — append; replaces `%root%`.
- `qd.path.is_absolute(path)`.
- `qd.env(name)`, `qd.exists(path)`, `qd.which(cmd)` (first match on `PATH`, or nil).
- `qd.fail(fmt, ...)` — raise a config error, formatted, without a `file:line:` prefix.
- `qd.check(value, validator, path)` — validate and return `value`; see below.
- `qd.schema` — validator combinators.
- `qd.warn(fmt, ...)`, `qd.debug(fmt, ...)` — stderr; `debug` only when `QD_DEBUG` is set.
- Hook phase only: `qd.run(cmd, ...)` (streams, errors on non-zero), `qd.exec(cmd, ...)` (captures stdout), `qd.write(path, content)`.
- `require("qd.<name>")` returns a built-in plugin; `require("x.y")` loads `<repo>/x/y.lua`.

A validator is anything callable as `(value, path) -> value` that raises on failure, so a plain function is one. `qd.schema` builds them:

| Combinator | Accepts |
|---|---|
| `string()`, `number()`, `boolean()` | that Lua type |
| `any()` | anything, nil included |
| `list(item)` | a list whose elements satisfy `item` |
| `table { field = v, ... }`, alias `record` | exactly those fields, unknown keys rejected |
| `map(item)` | any string keys, values satisfying `item` |
| `optional(inner)` | `inner`, or absent |
| `enum(...)` | one of those values |
| `one_of(...)` | a union of validators |

```lua
local s = qd.schema
local ENTRY = s.one_of(s.string(), s.table { name = s.string(), tap = s.optional(s.string()) })
qd.check(list, s.list(ENTRY), "brew")
```

Errors name the failing path, so `nushell.source[2]` and `brew[1].name` say so themselves. `one_of` surfaces an alternative's own error when only that one accepts the value's type, so a typo in a record still reports `unknown field brew[1].taps` rather than saying nothing matched. Reach the combinators through the namespace: `s.table` and `s.string` would shadow the Lua stdlib modules if pulled into locals. Both helpers live in an embedded Lua prelude evaluated before the instruction budget is armed, so they cost a config nothing.

Sandbox: fresh `Lua` per file; only `base`, `string`, `table`, `math` opened; no `os`/`io`; instruction budget on load. `print` is redirected to stderr, because stdout carries `show --format json` and the `__complete` lists. Hook-phase functions raise outside hooks (phase flag on the host), including inside plugin functions. Plugins run in the root file's VM. Ship `qd.d.lua` type stubs for lua-language-server.

## Data flow

```
CLI args + env ──► Host { os, tags, base paths }
repo tree      ──► Discover */qd.lua + root qd.lua
root qd.lua    ──► Plugins (declared, or built-in nushell/brew/scoop)
Host + files   ──► Lua load (pure) ──► plugin.resolve per key ──► Module { src, dest, globs, files, include, ext, hooks }
Module         ──► Planner: scan src, scan dest, diff by content hash ──► Plan { first_run, ops }
Plan           ──► status / --dry-run (print)   |   Apply (fs + age) ──► setup.before / setup.after
Modules        ──► plugin.compile ──► Output { path, content } ──► diff, atomic write, journal (same run)
Modules        ──► plugin.packages.* ──► argv lists ──► print / run
```

Core types:

```rust
struct Host { os: Os, tags: BTreeSet<String>, home: PathBuf, config: PathBuf, cache: PathBuf, /* ... */ }

struct Module {
    name: String, src: PathBuf, dest: Option<PathBuf>,
    ignore: GlobSet, encrypt: GlobSet,
    files: Vec<FilePair>, include: Vec<PathBuf>,
    ext: BTreeMap<String, serde_json::Value>,  // plugin keys, after `resolve`
    hooks: Hooks,            // keeps the Lua state + owned Function handles
}

struct Plugin { name, available, resolve, compile, packages: { list, install, upgrade } }  // Lua functions in the root VM
struct Output { plugin: String, path: PathBuf, content: String }

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

- `journal.jsonl` — one line per applied op, compiled file and hook run.
- `trash/<run>/<module or plugin>/<index>/<name>` — removed and overwritten files; `qd undo` restores the last apply, `qd trash prune --older 30d`.
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
| `qd packages install\|upgrade\|list [--manager <plugin>] [--dry-run]` | first available package plugin unless named |
| `qd compile [--dry-run]` | run every plugin's compile step (built-in: the two Nushell files) |
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
