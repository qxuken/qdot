# Converted dotfiles configs (migration phase 1)

One `qd.lua` per module of the `dotfiles` repo plus the root file, hand-converted
from `qd-config.yml` / `qd-*init*.nu` on 2026-09-04 and verified on macOS:
`qd show` matches the Nushell tool's resolution, `qd status` against the live
home shows only real drift, and a `qd push --no-setup` into a scratch home
lands every file (age files decrypt with the real key) with a clean status in
both directions afterwards.

Copy these into the scratch clone of dotfiles in phase 1:

    cp -R migration/dotfiles/ /path/to/dotfiles-scratch/

Applied to the live repo on 2026-09-06; these files are now a reference copy of
what landed there. Three things changed during that pass:

- The root `ignore` also hides `**/qd-init.nu` and `**/qd-pre-init.nu`, not just
  `**/qd-config.yml` — they were being copied into destinations, where qd has no
  use for them. All three go away in phase 4.
- `lazygit` ignores `**/github_pull_requests.json`; it is state, like `state.yml`.
- The root `nushell` block now sources the vendored `qd.nu` instead of including
  `dotfiles.nu`, and no longer sources `config.nu` — see the phase 3 notes in
  MIGRATION.md for why.

Note:
- Machines without brew or scoop package managers: Ubuntu appends `xclip` and
  `xsel` via `qd.host.ubuntu`, which needs `/etc/os-release` `ID=ubuntu`.
