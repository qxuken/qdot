-- Built-in plugin: Scoop. Folds every module's `scoop` list; packages are
-- installed as `bucket/name`, bucket `main` unless given.
--
--   scoop = { "git", { name = "lazygit", bucket = "extras" } }
local qd = require("qd")

local s = qd.schema
local SPEC = s.list(s.one_of(
  s.string(),
  s.table { name = s.string(), bucket = s.optional(s.string()) }
))

--- Split a validated entry into its name and bucket.
local function entry(p)
  if type(p) == "string" then
    return p, "main"
  end
  return p.name, p.bucket or "main"
end

local function collect(ctx)
  local packages, sources, seen, seen_src = {}, {}, {}, {}
  for _, m in ipairs(ctx.modules) do
    for _, p in ipairs(m.scoop or {}) do
      local name, bucket = entry(p)
      if not seen[name] then
        seen[name] = true
        if not seen_src[bucket] then
          seen_src[bucket] = true
          sources[#sources + 1] = bucket
        end
        packages[#packages + 1] = bucket .. "/" .. name
      end
    end
  end
  return { packages = packages, sources = sources }
end

local function cmd(verb, set)
  local c = { "scoop", verb }
  for _, p in ipairs(set) do
    c[#c + 1] = p
  end
  return c
end

return {
  name = "scoop",
  available = function() return qd.which("scoop") ~= nil end,
  resolve = function(m, list)
    if m.root then
      qd.fail("`scoop` belongs in modules, not the root qd.lua")
    end
    return qd.check(list, SPEC, "scoop")
  end,
  packages = {
    list = collect,
    install = function(ctx)
      local set, cmds = collect(ctx), {}
      if #set.packages == 0 then
        return cmds
      end
      for _, b in ipairs(set.sources) do
        cmds[#cmds + 1] = { "scoop", "bucket", "add", b }
      end
      cmds[#cmds + 1] = cmd("install", set.packages)
      return cmds
    end,
    upgrade = function(ctx)
      local set = collect(ctx)
      if #set.packages == 0 then
        return {}
      end
      return { cmd("update", set.packages) }
    end,
  },
}
