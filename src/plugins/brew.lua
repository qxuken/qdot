-- Built-in plugin: Homebrew. Folds every module's `brew` list.
--
--   brew = { "git", { name = "wezterm", tap = "wez/wezterm" } }
--
-- Kept separate from `scoop.lua` on purpose: each reads on its own and either
-- one is a template for a new package manager, as `contrib/apt.lua` shows.
local qd = require("qd")

local s = qd.schema
local SPEC = s.list(s.one_of(
  s.string(),
  s.table { name = s.string(), tap = s.optional(s.string()) }
))

--- Split a validated entry into its name and tap.
local function entry(p)
  if type(p) == "string" then
    return p, nil
  end
  return p.name, p.tap
end

local function collect(ctx)
  local packages, sources, seen, seen_src = {}, {}, {}, {}
  for _, m in ipairs(ctx.modules) do
    for _, p in ipairs(m.brew or {}) do
      local name, tap = entry(p)
      if not seen[name] then
        seen[name] = true
        if tap and not seen_src[tap] then
          seen_src[tap] = true
          sources[#sources + 1] = tap
        end
        packages[#packages + 1] = name
      end
    end
  end
  return { packages = packages, sources = sources }
end

local function cmd(verb, set)
  local c = { "brew", verb }
  for _, p in ipairs(set) do
    c[#c + 1] = p
  end
  return c
end

return {
  name = "brew",
  available = function() return qd.which("brew") ~= nil end,
  resolve = function(m, list)
    if m.root then
      qd.fail("`brew` belongs in modules, not the root qd.lua")
    end
    return qd.check(list, SPEC, "brew")
  end,
  packages = {
    list = collect,
    install = function(ctx)
      local set, cmds = collect(ctx), {}
      if #set.packages == 0 then
        return cmds
      end
      for _, tap in ipairs(set.sources) do
        cmds[#cmds + 1] = { "brew", "tap", tap }
      end
      cmds[#cmds + 1] = cmd("install", set.packages)
      return cmds
    end,
    upgrade = function(ctx)
      local set = collect(ctx)
      if #set.packages == 0 then
        return {}
      end
      return { cmd("upgrade", set.packages) }
    end,
  },
}
