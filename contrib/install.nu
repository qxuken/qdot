#!/usr/bin/env nu

# Portable installer for qd.
#
# Reads three files from a base URL — version.txt, checksums.sha256 and the
# binary for this platform — verifies the SHA-256, and installs into a
# user-writable directory. No admin rights, no compiler, no clone, and no
# forge API: any static host can serve that layout, which is what makes a
# future move survivable (see src/update.rs).
#
#   nu install.nu                                  # latest, into the default dir
#   nu install.nu --dest ~/bin                     # somewhere else
#   nu install.nu --version 0.3.0                  # a specific version
#   nu install.nu --base-url https://host/path     # a mirror, or a moved home
#   nu install.nu --init-url ssh://git@host/o/r.git  # …then set the machine up
#
# On a machine without nushell yet, fetch this script with the shell you have:
#
#   curl -fLO https://github.com/qxuken/qdot/releases/latest/download/install.nu
#
# For a base URL behind auth pass --token, or better, set QD_UPDATE_TOKEN so
# the token stays out of shell history and the process list.

# Where the current release's files are served.
const DEFAULT_BASE = "https://github.com/qxuken/qdot/releases/latest/download"
# Where one named version's files are served; `{version}` is substituted.
# Separate because GitHub spells "latest" in a different path position, so one
# template cannot cover both.
const DEFAULT_VERSION_BASE = "https://github.com/qxuken/qdot/releases/download/v{version}"

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
  if ($token | is-empty) { {} } else { {Authorization: $"Bearer ($token)"} }
}

# The version the base URL publishes, from its one-line version.txt.
def published-version [base: string, token: string]: nothing -> string {
  let url = $"($base)/version.txt"
  let body = try {
    http get --headers (auth-headers $token) --raw $url | decode utf-8
  } catch {|e|
    error make {msg: $"cannot read ($url): ($e.msg)"}
  }
  let v = (
    $body | lines | each {|l| $l | str trim} | where {|l| $l != ""}
    | get -o 0 | default "" | str replace --regex '^v' ''
  )
  # A host that answers every path with a page, rather than 404, would
  # otherwise have us install "<!doctype".
  if ($v | is-empty) or not ($v =~ '^[0-9]') {
    error make {msg: $"($url) does not contain a version", help: $"got: ($v)"}
  }
  $v
}

def fetch [url: string, token: string]: nothing -> binary {
  try {
    http get --headers (auth-headers $token) --raw $url
  } catch {|e|
    error make {msg: $"cannot download ($url): ($e.msg)"}
  }
}

export def main [
  --version: string             # version to install (default: whatever --base-url publishes)
  --dest: path                  # install directory (default: ~/.local/bin, or %LOCALAPPDATA%\qd\bin)
  --base-url: string = $DEFAULT_BASE # serves version.txt, checksums.sha256, the binaries
  --version-base-url: string = $DEFAULT_VERSION_BASE # same, for one named --version
  --token: string               # token for a base URL behind auth (or set QD_UPDATE_TOKEN)
  --init-url: string            # run `qd init --url <this>` after installing
  --keep                        # do not overwrite an existing binary
] {
  let token = ($token | default ($env.QD_UPDATE_TOKEN? | default ""))
  let asset = (asset-name)
  let dest = ($dest | default (default-dest) | path expand)
  let exe = if $nu.os-info.name == "windows" { "qd.exe" } else { "qd" }
  let target = ($dest | path join $exe)

  if $keep and ($target | path exists) {
    print $"($target) already exists, leaving it alone \(--keep)"
    return
  }

  # A named --version comes from its own directory; otherwise the base URL
  # publishes which version it is serving.
  let base = if ($version | is-empty) {
    $base_url | str trim --right --char '/'
  } else {
    $version_base_url | str replace --all "{version}" ($version | str replace --regex '^v' '')
      | str trim --right --char '/'
  }
  # Not `default`, which nushell evaluates eagerly: a named version is already
  # known, and its own directory need not serve a version.txt at all.
  let version = if ($version | is-empty) {
    published-version $base $token
  } else {
    $version | str replace --regex '^v' ''
  }
  print $"installing qd ($version) \(($asset)) from ($base) into ($dest)"

  # The checksums file is served as octet-stream, so decode it before parsing.
  # A 404 here almost always means that version is not published there.
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
