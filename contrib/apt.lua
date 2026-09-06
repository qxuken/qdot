-- Example third-party plugin: Debian/Ubuntu packages.
--
-- Copy it into your dotfiles repo as `plugins/apt.lua` and declare it in the
-- root qd.lua, which also has to name the built-ins you still want:
--
--   return {
--     plugins = { require("qd.nushell"), require("qd.brew"), require("plugins.apt") },
--   }
--
-- Modules then carry an `apt` list beside `brew`. On a machine without
-- apt-get, `available()` is false and `qd packages install` falls through to
-- the next manager.
local qd = require("qd")

local SPEC = qd.schema.list(qd.schema.string())

local function collect(ctx)
  local seen, out = {}, {}
  for _, m in ipairs(ctx.modules) do
    for _, p in ipairs(m.apt or {}) do
      if not seen[p] then
        seen[p] = true
        out[#out + 1] = p
      end
    end
  end
  return out
end

return {
  name = "apt",
  available = function() return qd.which("apt-get") ~= nil end,
  resolve = function(m, list)
    if m.root then
      qd.fail("`apt` belongs in modules, not the root qd.lua")
    end
    return qd.check(list, SPEC, "apt")
  end,
  packages = {
    list = collect,
    install = function(ctx)
      local pkgs = collect(ctx)
      if #pkgs == 0 then
        return {}
      end
      local install = { "sudo", "apt-get", "install", "-y" }
      for _, p in ipairs(pkgs) do
        install[#install + 1] = p
      end
      return { { "sudo", "apt-get", "update" }, install }
    end,
    upgrade = function(ctx)
      if #collect(ctx) == 0 then
        return {}
      end
      return { { "sudo", "apt-get", "update" }, { "sudo", "apt-get", "upgrade", "-y" } }
    end,
  },
}
