local qd = require("qd")

return {
  ignore = { "**/.git/**", "**/.DS_Store" },
  encrypt = { "**/*.p12" },
  include = { qd.path.dotfiles(".editorconfig") },
  nushell = {
    source = { qd.path.dotfiles("config.nu") },
  },
}
