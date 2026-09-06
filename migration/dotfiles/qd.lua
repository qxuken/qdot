local qd = require("qd")

return {
  ignore  = { "**/.git/**", "**/.gitignore", "**/.editorconfig", "**/.DS_Store", "**/qd-config.yml" }, -- qd-config.yml: legacy, drop after phase 4
  include = { qd.path.dotfiles(".editorconfig") },
  encrypt = { "**/*.p12" },
  nushell = {
    -- TODO(migration): replace with `use <qd repo>/contrib/qd.nu` once dotfiles.nu is gone
    include = { qd.path.dotfiles("dotfiles.nu") },
    source  = { qd.path.dotfiles("config.nu") },
  },
}
