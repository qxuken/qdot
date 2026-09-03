local qd = require("qd")

return {
  path  = qd.path.config("starship"),
  scoop = { { name = "starship", bucket = "main" } },
  brew  = { { name = "starship", tap = "some/tap" } },
  setup = {
    version = 3,
    before = function(m) end,
    after = function(m)
      local out = qd.exec("echo", "init", m.name)
      qd.write(qd.path.join(m.dest, "starship.nu"), out)
    end,
  },
}
