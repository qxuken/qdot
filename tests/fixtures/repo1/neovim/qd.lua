local qd = require("qd")
local lib = require("lib")
local pkgs = qd.list(lib.common_packages(), "neovim")

return {
  path  = qd.host.windows and qd.path.local_appdata("nvim") or qd.path.config("nvim"),
  brew  = qd.host.ubuntu and qd.list(pkgs, "xclip") or pkgs,
  scoop = pkgs,
  nushell = { source = { "config.nu" }, env_source = { "env.nu" } },
  setup = {
    after = function(m)
      qd.write(qd.path.join(m.dest, "generated.txt"), "hello " .. m.name)
    end,
  },
}
