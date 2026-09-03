local qd = require("qd")

return {
  path   = qd.path.cache("zoxide"),
  brew   = { "zoxide" },
  scoop  = { "zoxide" },
  ignore = { "**/zoxide.nu" },
  dotfile = { source = { "zoxide.nu" } },
  setup = {
    version = 1,
    after = function(m)
      qd.write(qd.path.join(m.dest, "zoxide.nu"), qd.exec("zoxide", "init", "nushell"))
    end,
  },
}
