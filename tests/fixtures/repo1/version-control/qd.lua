local qd = require("qd")

return {
  path  = qd.path.cache("version-control"),
  files = {
    { src = "gitconfig", dest = qd.path.home(".gitconfig"), enabled = not qd.tag("work") },
    { src = "always", dest = qd.path.home(".always") },
  },
  dotfile = { source = { "config.nu" } },
}
