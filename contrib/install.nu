#!/usr/bin/env nu

# Portable installer for qd.
#
# Downloads a released binary from the Forgejo generic package registry,
# verifies its SHA-256 against the published checksums file, and installs it
# into a user-writable directory. No admin rights, no compiler, no clone.
#
#   nu install.nu                                  # latest, into the default dir
#   nu install.nu --version 0.1.0                  # a specific version
#   nu install.nu --dest ~/bin                     # somewhere else
#   nu install.nu --init-url ssh://git@host/o/r.git  # …then set the machine up
#
# On a machine without nushell yet, fetch this script with the shell you have:
#
#   curl -fLO https://drydock9.qxuken.dev/api/packages/qxuken/generic/qd/0.1.0/install.nu
#
# For a private registry pass --token, or better, set QD_UPDATE_TOKEN so the
# token stays out of shell history and the process list.

const DEFAULT_HOST = "https://drydock9.qxuken.dev"
const DEFAULT_OWNER = "qxuken"
const PACKAGE = "qd"

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
  if ($token | is-empty) { {} } else { {Authorization: $"token ($token)"} }
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

def latest-version [host: string, owner: string, token: string]: nothing -> string {
  let url = $"($host)/api/v1/packages/($owner)?type=generic&q=($PACKAGE)&limit=100"
  let packages = try {
    http get --headers (auth-headers $token) $url
  } catch {|e|
    error make {msg: $"cannot reach the package registry at ($url): ($e.msg)"}
  }
  let versions = ($packages | where name == $PACKAGE | get version)
  if ($versions | is-empty) {
    error make {msg: $"no published versions of `($PACKAGE)` at ($url)"}
  }
  $versions | each {|v| version-parts $v} | sort-by major minor patch rel | last | get version
}

def fetch [url: string, token: string]: nothing -> binary {
  try {
    http get --headers (auth-headers $token) --raw $url
  } catch {|e|
    error make {msg: $"cannot download ($url): ($e.msg)"}
  }
}

export def main [
  --version: string             # version to install (default: the newest published)
  --dest: path                  # install directory (default: ~/.local/bin, or %LOCALAPPDATA%\qd\bin)
  --host: string = $DEFAULT_HOST # Forgejo base URL
  --owner: string = $DEFAULT_OWNER # package owner
  --token: string               # registry token (or set QD_UPDATE_TOKEN)
  --init-url: string            # run `qd init --url <this>` after installing
  --keep                        # do not overwrite an existing binary
] {
  let token = ($token | default ($env.QD_UPDATE_TOKEN? | default ""))
  let host = ($host | str trim --right --char '/')
  let asset = (asset-name)
  let dest = ($dest | default (default-dest) | path expand)
  let exe = if $nu.os-info.name == "windows" { "qd.exe" } else { "qd" }
  let target = ($dest | path join $exe)

  if $keep and ($target | path exists) {
    print $"($target) already exists, leaving it alone \(--keep)"
    return
  }

  let version = ($version | default (latest-version $host $owner $token))
  let base = $"($host)/api/packages/($owner)/generic/($PACKAGE)/($version)"
  print $"installing qd ($version) \(($asset)) into ($dest)"

  # The checksums file is served as octet-stream, so decode it before parsing.
  # A 404 here almost always means the version itself is not published.
  let sums = try {
    http get --headers (auth-headers $token) --raw $"($base)/checksums.sha256"
  } catch {|e|
    error make {msg: $"qd ($version) is not published at ($base)", help: $"($e.msg)"}
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
  fetch $"($base)/($asset)" $token | save --raw --force $staged

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
