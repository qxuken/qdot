#!/usr/bin/env nu

# Portable installer for qd.
#
# Downloads a released binary from the GitHub release assets, verifies its
# SHA-256 against the published checksums file, and installs it into a
# user-writable directory. No admin rights, no compiler, no clone.
#
#   nu install.nu                                  # latest, into the default dir
#   nu install.nu --version 0.3.0                  # a specific version
#   nu install.nu --dest ~/bin                     # somewhere else
#   nu install.nu --init-url ssh://git@host/o/r.git  # …then set the machine up
#
# On a machine without nushell yet, fetch this script with the shell you have:
#
#   curl -fLO https://github.com/qxuken/qdot/releases/latest/download/install.nu
#
# For a private repository pass --token, or better, set QD_UPDATE_TOKEN so the
# token stays out of shell history and the process list. The token reaches the
# API only: asset URLs redirect to a signed CDN URL that rejects it.

const DEFAULT_API = "https://api.github.com"
const DEFAULT_OWNER = "qxuken"
const DEFAULT_REPO = "qdot"

# Asset published for this platform. The names match Rust's OS/ARCH constants,
# which is exactly what `$nu.os-info` reports.
def asset-name []: nothing -> string {
  let os = $nu.os-info.name
  let arch = $nu.os-info.arch
  if $os not-in ["macos" "linux" "windows"] or $arch not-in ["aarch64" "x86_64"] {
    error make {msg: $"no qd build for ($os)/($arch)"}
  }
  if $os == "windows" and $arch != "x86_64" {
    error make {msg: $"no qd build for windows/($arch)"}
  }
  let ext = if $os == "windows" { ".exe" } else { "" }
  $"qd-($os)-($arch)($ext)"
}

def default-dest []: nothing -> path {
  if $nu.os-info.name == "windows" {
    $nu.home-dir | path join "AppData" "Local" "qd" "bin"
  } else {
    $nu.home-dir | path join ".local" "bin"
  }
}

def auth-headers [token: string]: nothing -> record {
  let base = {
    Accept: "application/vnd.github+json"
    "X-GitHub-Api-Version": "2022-11-28"
  }
  if ($token | is-empty) { $base } else { $base | merge {Authorization: $"Bearer ($token)"} }
}

# Sortable pieces of a version; a prerelease ranks below its release.
def version-parts [v: string]: nothing -> record {
  let t = ($v | str replace --regex '^v' '')
  let split = ($t | split row '-')
  let nums = ($split | first | split row '.' | each {|x| try { $x | into int } catch { 0 }})
  {
    version: $v
    major: ($nums | get -o 0 | default 0)
    minor: ($nums | get -o 1 | default 0)
    patch: ($nums | get -o 2 | default 0)
    rel: (if ($split | length) > 1 { 0 } else { 1 })
  }
}

# Drafts are invisible without a token and ranked out with one; a prerelease
# ranks below the release of the same version, so it only wins when it is the
# newest thing published.
def latest-version [api: string, owner: string, repo: string, token: string]: nothing -> string {
  let url = $"($api)/repos/($owner)/($repo)/releases?per_page=100"
  let releases = try {
    http get --headers (auth-headers $token) $url
  } catch {|e|
    error make {msg: $"cannot reach the releases API at ($url): ($e.msg)"}
  }
  let tags = ($releases | where not ($it.draft? | default false) | get tag_name)
  if ($tags | is-empty) {
    error make {msg: $"no published releases at ($url)"}
  }
  let newest = ($tags | each {|v| version-parts $v} | sort-by major minor patch rel | last)
  $newest.version | str replace --regex '^v' ''
}

# No token: an asset URL redirects to a signed CDN URL, which rejects a
# request that also carries an Authorization header.
def fetch [url: string]: nothing -> binary {
  try {
    http get --raw $url
  } catch {|e|
    error make {msg: $"cannot download ($url): ($e.msg)"}
  }
}

export def main [
  --version: string             # version to install (default: the newest released)
  --dest: path                  # install directory (default: ~/.local/bin, or %LOCALAPPDATA%\qd\bin)
  --api: string = $DEFAULT_API  # GitHub API base
  --owner: string = $DEFAULT_OWNER # repository owner
  --repo: string = $DEFAULT_REPO # repository name
  --token: string               # token for a private repository (or set QD_UPDATE_TOKEN)
  --init-url: string            # run `qd init --url <this>` after installing
  --keep                        # do not overwrite an existing binary
] {
  let token = ($token | default ($env.QD_UPDATE_TOKEN? | default ""))
  let api = ($api | str trim --right --char '/')
  let asset = (asset-name)
  let dest = ($dest | default (default-dest) | path expand)
  let exe = if $nu.os-info.name == "windows" { "qd.exe" } else { "qd" }
  let target = ($dest | path join $exe)

  if $keep and ($target | path exists) {
    print $"($target) already exists, leaving it alone \(--keep)"
    return
  }

  let version = ($version | default (latest-version $api $owner $repo $token)
    | str replace --regex '^v' '')
  # The download endpoint takes the tag, which carries the `v`.
  let base = $"https://github.com/($owner)/($repo)/releases/download/v($version)"
  print $"installing qd ($version) \(($asset)) into ($dest)"

  # The checksums file is served as octet-stream, so decode it before parsing.
  # A 404 here almost always means the version itself has no release.
  let sums = try {
    http get --raw $"($base)/checksums.sha256"
  } catch {|e|
    error make {msg: $"qd ($version) has no release at ($base)", help: $"($e.msg)"}
  }
  let expected = (
    $sums
    | decode utf-8
    | lines
    | parse --regex '(?<hash>[0-9a-f]{64})\s+\*?(?<name>.+)'
    | where name == $asset
    | get -o 0.hash
  )
  if ($expected | is-empty) {
    error make {msg: $"checksums.sha256 for ($version) has no entry for ($asset)"}
  }

  # Stage next to the target so the final move is a rename on the same volume.
  mkdir $dest
  let staged = ($dest | path join $".qd-install-($version).tmp")
  fetch $"($base)/($asset)" | save --raw --force $staged

  let actual = (open --raw $staged | hash sha256)
  if $actual != $expected {
    rm --force $staged
    error make {msg: $"checksum mismatch for ($asset): expected ($expected), got ($actual)"}
  }
  print $"verified sha256 ($actual)"

  if $nu.os-info.family == "unix" {
    ^chmod +x $staged
  }
  # Windows refuses to overwrite a running executable; removing first turns a
  # confusing rename failure into a clear one.
  rm --force $target
  mv --force $staged $target

  let reported = (do { ^$target --version } | complete)
  if $reported.exit_code != 0 {
    error make {msg: $"installed ($target) but it does not run: ($reported.stderr | str trim)"}
  }
  print $"installed ($reported.stdout | str trim) at ($target)"

  if ($env.PATH | where {|p| ($p | path expand) == $dest} | is-empty) {
    print ""
    print $"($dest) is not on PATH. Add it, for this session:"
    if $nu.os-info.name == "windows" {
      print $"  $env.Path = \($env.Path | prepend '($dest)')"
    } else {
      print $"  $env.PATH = \($env.PATH | prepend '($dest)')"
    }
  }

  if ($init_url | is-not-empty) {
    print ""
    print $"running qd init --url ($init_url)"
    ^$target init --url $init_url
  }
}
