-- Built-in plugin: fold every module's `nushell` table into the two entry
-- files that config.nu / env.nu source.
--
--   nushell = {
--     include     = { "vpn.nu" },     -- `use` lines in ~/.dotfiles.local.nu
--     source      = { "config.nu" },  -- `source` lines
--     env_include = {},               -- `use` lines in ~/.dotfiles-env.local.nu
--     env_source  = { "env.nu" },     -- `source` lines
--   }
--
-- Relative entries resolve against the module's `path`; the root file must use
-- absolute paths (`qd.path.dotfiles(...)`).
local qd = require("qd")

local MAIN = ".dotfiles.local.nu"
local ENV = ".dotfiles-env.local.nu"
local FIELDS = { "include", "source", "env_include", "env_source" }

local s = qd.schema
local PATHS = s.optional(s.list(s.string()))
local SPEC = s.table {
  include = PATHS,
  source = PATHS,
  env_include = PATHS,
  env_source = PATHS,
}

local function resolve(m, cfg)
  qd.check(cfg, SPEC, "nushell")
  local out = {}
  for _, field in ipairs(FIELDS) do
    local abs = {}
    for i, p in ipairs(cfg[field] or {}) do
      if qd.path.is_absolute(p) then
        abs[i] = p
      elseif m.dest then
        abs[i] = qd.path.join(m.dest, p)
      else
        qd.fail(
          "`nushell.%s` entry `%s` is relative but there is no `path` to resolve it against",
          field, p
        )
      end
    end
    out[field] = abs
  end
  return out
end

local function add(cfg, main, env)
  local function lines(list, verb, acc)
    for _, p in ipairs(list or {}) do
      acc[#acc + 1] = verb .. " `" .. p .. "`"
    end
  end
  lines(cfg.include, "use", main)
  lines(cfg.source, "source", main)
  lines(cfg.env_include, "use", env)
  lines(cfg.env_source, "source", env)
end

-- Unique, reverse sorted, so `use` lines come before `source` lines. Same
-- ordering as the Nushell tool this replaced.
local function finish(acc)
  local seen, uniq = {}, {}
  for _, l in ipairs(acc) do
    if not seen[l] then
      seen[l] = true
      uniq[#uniq + 1] = l
    end
  end
  table.sort(uniq, function(a, b) return a > b end)
  if #uniq == 0 then
    return ""
  end
  return table.concat(uniq, "\n") .. "\n"
end

return {
  name = "nushell",
  resolve = resolve,
  compile = function(ctx)
    local main, env = {}, {}
    for _, m in ipairs(ctx.modules) do
      if m.nushell then add(m.nushell, main, env) end
    end
    if ctx.global.nushell then add(ctx.global.nushell, main, env) end
    return {
      { path = qd.path.home(MAIN), content = finish(main) },
      { path = qd.path.home(ENV), content = finish(env) },
    }
  end,
}
