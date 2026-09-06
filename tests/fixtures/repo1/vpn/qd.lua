local qd = require("qd")

return {
  path    = qd.path.home("vpn"),
  brew    = { "openconnect" },
  scoop   = { "openconnect" },
  encrypt = { "**/config.yml" },
  ignore  = { "**/history.txt" },
  nushell = { include = { "vpn.nu" } },
}
