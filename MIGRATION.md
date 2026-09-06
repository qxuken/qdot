# Migration: dotfiles.nu + fossil → qd + git

Order of operations. Nothing in the live dotfiles repo changes until phase 3; phases 1 and 2 happen against copies.

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

1. Clone the current dotfiles repo to a scratch directory. `master.key` is copied in by hand.
2. Copy the already converted files from `migration/dotfiles/` (root plus all 13 modules) into the scratch clone. Keep the YAML and `qd-*init*.nu` files in place for now; the root `ignore` hides `qd-config.yml` from qd. Note the Nushell tool does *not* ignore `qd.lua`: its next `push` would copy the Lua files into destinations, harmlessly. Do not run the old tool from the scratch clone.
3. For each module compare `qd show <module>` against the Nushell resolution. Fix the Lua until they match.
4. Port each `qd-init.nu` into `setup.after`. Starship's generator, fnm and uv installs, nushell's `touch` of the local files. Set `setup.version = 1` on all of them. Files a hook generates into `path` (e.g. `starship.nu`) go into that module's `ignore`, otherwise the next pull commits them.
5. `files[].src` is now relative to the module directory in the repo, not to `path`. `version-control` becomes `files = { { src = ".gitconfig", dest = qd.path.home(".gitconfig"), enabled = not qd.tag("work") } }`.
6. `qd status` from the scratch clone against the real `HOME` should list only files that already differ today. Anything else is a conversion bug.

## Phase 2 — fossil → git on Forgejo

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

Per machine:

1. Install `qd` (download from Forgejo release or `cargo install --path`), confirm `qd --version`.
2. `qd init --url ssh://git@drydock9-port1.qxuken.dev/qxuken/dotfiles.git --path ~/dotfiles` into the existing dotfiles path, or point `state.toml` at it if the clone is already there. Copy `master.key` in.
3. `qd tag add <tag>` for whatever `DOTFILES_TAGS` was set to on that machine.
4. `qd state adopt` so existing modules are marked initialized at `setup.version` 1.
5. `qd status`: must be empty, or list only differences you recognise. Do not proceed if it wants to remove anything unexpected.
6. `qd push` (all modules), which also compiles. Use `--no-setup` if the setup hooks (fnm, uv, starship, zoxide generators) already ran on this machine, then `qd state adopt`. Check `~/.dotfiles.local.nu` no longer references `dotfiles.nu`.
7. Replace the aliases in the repo's `config.nu` (`dpha` etc.) with `qd` invocations, or `use` `contrib/qd.nu` from the qd repo which defines `dph`, `dpl`, `dst` and completions.
8. Open a fresh shell, confirm prompt, completions, and one `qd pull -s "test"` round trip through git.

## Phase 4 — cleanup, after the last machine

One commit in dotfiles removing: `dotfiles.nu`, every `qd-config.yml`, every `qd-*init*.nu`, `global-config.yml`, `.fossil-settings/`. Delete `repo.fossil` and `.fslckout` locally, then retire the fossil server at `dotfiles.qxuken.dev`.

Delete `qd state adopt` from qd one release later.

## Rollback

- Before phase 3 on a machine: nothing to undo, the Nushell tool still works and fossil is unchanged.
- During phase 3: `fossil update` restores the tree, the old aliases still exist until step 7, and qd's trash holds anything it removed.
- After phase 4: git history has the YAML if the Lua ever needs cross-checking.
