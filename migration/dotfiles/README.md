# Converted dotfiles configs (migration phase 1)

One `qd.lua` per module of the `dotfiles` repo plus the root file, hand-converted
from `qd-config.yml` / `qd-*init*.nu` on 2026-09-04 and verified on macOS:
`qd show` matches the Nushell tool's resolution, `qd status` against the live
home shows only real drift, and a `qd push --no-setup` into a scratch home
lands every file (age files decrypt with the real key) with a clean status in
both directions afterwards.

Copy these into the scratch clone of dotfiles in phase 1:

    cp -R migration/dotfiles/ /path/to/dotfiles-scratch/

Notes:
- The root `ignore` includes `**/qd-config.yml` so the legacy files are not
  pushed to machines; drop it in phase 4.
- The root `dotfile.include` still references `dotfiles.nu`; replace with a
  `use` of `contrib/qd.nu` when the Nushell module is deleted.
- `lazygit` has an untracked `github_pull_requests.json` in its destination on
  this machine; add `**/github_pull_requests.json` to its `ignore` if it is
  state rather than config.
- Machines without brew or scoop package managers: Ubuntu appends `xclip` and
  `xsel` via `qd.host.ubuntu`, which needs `/etc/os-release` `ID=ubuntu`.
