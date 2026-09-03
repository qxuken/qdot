local qd = require("qd")

return {
  enabled = not qd.tag("wsl"),
  path    = qd.path.config("wezterm"),
  encrypt = { "**/fonts/*.ttf" },
}
