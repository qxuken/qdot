# Migration: dotfiles.nu + fossil → qd + git

Order of operations. Phases 1 and 2 happen against copies; phase 2 then installs the
resulting history into the live repo, and phase 3 is the first thing to touch the
machine.

## Phase 0 — qd is correct (this repo)

Status 2026-09-04: every criterion below is met on macOS (34 tests, plus a real-data push of the converted dotfiles into a scratch home with `--no-setup`). Linux and Windows are untested.

Exit criteria before touching real data:

- `qd show <module>` resolves every current module's config identically to what `dotfiles verify-config` prints today (paths, packages, globs, nushell entries), on macOS at least.
- `qd status` against a scratch clone reports zero ops after a `push --dry-run` followed by a real push into a scratch `HOME`.
- Round trip: `push` into scratch HOME, edit a file there, `pull` back, encrypted file included, `status` empty again.
- Remove path: delete a file on one side, confirm `status` shows it distinctly, `--no-remove` skips it, a real run moves it to trash, `undo` restores it.
- Setup hooks: `setup.version` bump reruns `after` on the next push, `--force` reruns it, nothing reruns otherwise.
- Integration tests in `tests/` cover all of the above with fixture modules and overridden `HOME` / `QD_STATE`.

Test harness: every test gets a temp dir with `repo/`, `home/`, `state/`; `Host` is constructed directly from those paths, never from the real environment.

## Phase 1 — convert configs (in a scratch clone of dotfiles)

Status 2026-09-06: done. Every module's `qd show` matches `dotfiles verify-config`
(dest, brew, scoop, ignore, encrypt, nushell entries, files), a push into a scratch
HOME applies 175 operations and leaves `status` empty in both directions, all five
`.age` files decrypt, and the compiled `~/.dotfiles.local.nu` came out byte-identical
to the one the Nushell tool had generated. Two ignore entries were added during
verification: `**/qd-init.nu` / `**/qd-pre-init.nu` on the root (they were being
copied into destinations, where qd has no use for them) and
`**/github_pull_requests.json` on lazygit (state, like `state.yml`).

1. Clone the current dotfiles repo to a scratch directory. `master.key` is copied in by hand.
2. Copy the already converted files from `migration/dotfiles/` (root plus all 13 modules) into the scratch clone. Keep the YAML and `qd-*init*.nu` files in place for now; the root `ignore` hides `qd-config.yml` from qd. Note the Nushell tool does *not* ignore `qd.lua`: its next `push` would copy the Lua files into destinations, harmlessly. Do not run the old tool from the scratch clone.
3. For each module compare `qd show <module>` against the Nushell resolution. Fix the Lua until they match.
4. Port each `qd-init.nu` into `setup.after`. Starship's generator, fnm and uv installs, nushell's `touch` of the local files. Set `setup.version = 1` on all of them. Files a hook generates into `path` (e.g. `starship.nu`) go into that module's `ignore`, otherwise the next pull commits them.
5. `files[].src` is now relative to the module directory in the repo, not to `path`. `version-control` becomes `files = { { src = ".gitconfig", dest = qd.path.home(".gitconfig"), enabled = not qd.tag("work") } }`.
6. `qd status` from the scratch clone against the real `HOME` should list only files that already differ today. Anything else is a conversion bug.

## Phase 2 — fossil → git on Forgejo

Status 2026-09-06: done. Note `~/dotfiles` already held a `.git` — the old
`fossil git export` mirror, stale at 62 of 74 check-ins, autopushing to
github.com/qxuken/dotfiles. It was backed up and replaced by the fresh export
below, so the two histories have different SHAs and the GitHub mirror is now
orphaned. Forgejo has 76 commits: 74 check-ins, the `.gitignore` fix, and the
`qd.lua` files. The repo was created by push-to-create rather than by hand.

1. Create `qxuken/dotfiles` on Forgejo, empty, no README.
2. Export:

   ```bash
   fossil export --git --rename-trunk main repo.fossil | git -C /path/to/scratch fast-import
   ```

3. Translate `.fossil-settings/ignore-glob` into `.gitignore`. Confirm `master.key`, `repo.fossil`, `.fslckout`, `.mirror_state` are ignored.
4. Sanity-check the export: file count, all five `.age` files decrypt with `master.key`, `git log` reaches the earliest fossil commit.
5. Add the converted `qd.lua` files from phase 1 as one commit on top, push to Forgejo.
6. Optional: Forgejo push mirror to `github.com/qxuken/dotfiles` if the public mirror stays.

Fossil stays untouched and remains the source of truth until phase 3 is done on every machine.

## Phase 3 — cut over machines, one at a time

Order: this macOS first, then Ubuntu and WSL, Windows last.

Status 2026-09-07: macOS, WSL (Ubuntu-24.04 on the Windows box) and Windows done.
Only the Ubuntu machine still runs the Nushell tool against fossil. **The two
histories are now forked**: the neovim and `.gitconfig` drift below was pulled into
git only, Windows' `wezterm/config/tabbar.lua` drift went to git only (commit
`7b61db8`), and anything committed from another machine lands in fossil only. Cut
Ubuntu over before the gap grows, or re-run the phase 1 comparison there first.

What the WSL and Windows cutovers taught (2026-09-07):

- Neither machine set `DOTFILES_TAGS`, so step 3 was a no-op on both. Both fossil
  checkouts sat at the same May 24 check-in, so `qd status` from a scratch clone
  listed exactly the macOS drift already in git (four neovim files, `keybinds.nu`,
  `.gitconfig`). The drifted destination files were diffed against the fossil tree
  first to prove they carried no local edits; WSL had none, Windows had two:
  `lazy-lock.json` (Jul 15, older than the macOS version in git, so git won) and
  `tabbar.lua` (Feb 9, only edited copy anywhere, so `qd pull wezterm -s` first,
  then `qd push`).
- Converting the tree in place: `git clone --no-checkout` to a temp dir, move its
  `.git` into `~/dotfiles`, `git reset --hard`, then `git clean -n`. The fossil tree
  leaves files git HEAD has since deleted (`neovim/.../neotest.lua`) as untracked
  leftovers, and qd scans the filesystem, not git, so they mask the removal until
  `git clean -f` drops them.
- Windows: scoop's git ships `core.autocrlf = true` in its system gitconfig, which
  checks everything out CRLF and makes every file "differ". `git config
  core.autocrlf false` in the repo before the reset fixes it. Fossil stored a fair
  number of `.lua`/`.nu`/`.toml` files with CRLF already, so a `.gitattributes` and
  a normalising commit is a candidate for phase 4. The fossil checkout file on
  Windows is `_FOSSIL_`, not `.fslckout`; it went into `.git/info/exclude`.
- Windows: `std::fs::canonicalize` yields `\\?\C:\...`, which lands in
  `state.toml` and in the compiled `~/.dotfiles.local.nu` `source` line. Nushell
  copes, but `dunce::canonicalize` (or stripping the prefix) would be cleaner.
- Machine-local git settings moved to `~/.gitconfig.local`: WSL had the linuxbrew
  credential-manager helper plus dev.azure.com/drydock9 credential sections,
  Windows the drydock9 credential section and `[gui] recentrepo`.
- Backups of both pre-cutover trees: `~/qd-mig-backup/before` on WSL and the
  session scratchpad `before/` on Windows; the fossil checkouts still work too.

Three things worth knowing before the next machine:

- WSL needs no tag. `qd.host.wsl` is detected from `WSL_DISTRO_NAME`, and
  `wezterm` keys off that directly, so step 3 below only covers tags the machine
  actually declares (`personal`, `work`). qd up to 0.1.1 also copied `wsl` into the
  tag set; that is gone, and since nothing reads the tag any more, running an older
  binary on the WSL machine makes no difference.

- The Nushell tool copied with `cp --update`, so any destination file edited after
  the last push had silently stopped syncing. On macOS that was four neovim files
  and `~/.gitconfig` (git-lfs plus credential-manager sections), drifting for
  months. qd compares content and would have overwritten all of them. **Run
  `qd status` and `qd status --pull` and reconcile before the first `qd push`.**
- `qd.nu` is vendored at the repo root and is `source`d, not `use`d — `use` would
  namespace the aliases as `qd dph`, which collides with the binary. It replaces
  `config.nu` in the root config's `nushell`; `config.nu` keeps the old
  `dotfiles ...` aliases for machines that have not cut over, reaching them through
  `global-config.yml`. `dpha`/`dpla` are gone, `dph`/`dpl` already act on every
  module.

Per machine:

1. Install `qd` (download from Forgejo release or `cargo install --path`), confirm `qd --version`.
2. `qd init --url ssh://git@drydock9-port1.qxuken.dev/qxuken/dotfiles.git --path ~/dotfiles` into the existing dotfiles path, or point `state.toml` at it if the clone is already there. Copy `master.key` in.
3. `qd tag add <tag>` for whatever `DOTFILES_TAGS` was set to on that machine — that
   writes `state.toml`, which is what you want, though qd also reads `QD_TAGS` and
   `DOTFILES_TAGS` straight from the environment. Not `wsl`: that one is detected.
4. `qd state adopt` so existing modules are marked initialized at `setup.version` 1.
5. `qd status`: must be empty, or list only differences you recognise. Do not proceed if it wants to remove anything unexpected.
   If the machine has git settings of its own, put them in `~/.gitconfig.local` first —
   the shared `.gitconfig` includes it last, and the push overwrites `~/.gitconfig`.
6. `qd push` (all modules), which also compiles. Use `--no-setup` if the setup hooks (fnm, uv, starship, zoxide generators) already ran on this machine, then `qd state adopt`. Check `~/.dotfiles.local.nu` no longer references `dotfiles.nu`.
7. Replace the aliases in the repo's `config.nu` (`dpha` etc.) with `qd` invocations, or `use` `contrib/qd.nu` from the qd repo which defines `dph`, `dpl`, `dst` and completions.
8. Open a fresh shell, confirm prompt, completions, and one `qd pull -s "test"` round trip through git.

## Phase 4 — cleanup, after the last machine

One commit in dotfiles removing: `dotfiles.nu`, `config.nu` (the old `dotfiles ...`
aliases), every `qd-config.yml`, every `qd-*init*.nu`, `global-config.yml`,
`.fossil-settings/`. Then trim the root `qd.lua` ignore list back to its four generic
entries, since the legacy files it hides are gone, and drop `DOTFILES_TAGS` from
`Host::detect` in qd, leaving `QD_TAGS`. Delete `repo.fossil` and `.fslckout` locally, then retire the fossil server at `dotfiles.qxuken.dev`.

Delete `qd state adopt` from qd one release later.

## Rollback

- Before phase 3 on a machine: nothing to undo, the Nushell tool still works and fossil is unchanged.
- During phase 3: `fossil revert` restores the tree, the old aliases still exist until
  step 7, and qd's trash holds anything it removed. `repo.fossil` and `.fslckout` are
  still in place and gitignored, so the fossil checkout keeps working alongside git.
  The replaced GitHub mirror `.git` is reconstructible at any time with
  `fossil git export` into a fresh directory.
- After phase 4: git history has the YAML if the Lua ever needs cross-checking.
