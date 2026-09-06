-- Evaluated once per VM, before the instruction budget is armed. Returns the
-- helpers the host installs onto the `qd` table, plus the `print` replacement.
--
-- Everything here is shared by config files and plugins: without it every
-- plugin rewrites the same argument checking and the same error formatting.
local log = ...

local M = {}

local function format(fmt, ...)
  if type(fmt) ~= "string" then
    error("expected a format string, got " .. type(fmt), 0)
  end
  -- No arguments means no formatting, so a literal `%` stays literal.
  if select("#", ...) == 0 then
    return fmt
  end
  return fmt:format(...)
end

--- Raise a config error: formatted, and without a `file:line:` prefix, because
--- the message already says which field is wrong.
local function fail(fmt, ...)
  error(format(fmt, ...), 0)
end
M.fail = fail

function M.warn(fmt, ...)
  log("warn", format(fmt, ...))
end

--- Only shown when QD_DEBUG is set.
function M.debug(fmt, ...)
  log("debug", format(fmt, ...))
end

--- `print` writes to stdout, which carries `qd show --format json` and the
--- shell completion lists. Route it to stderr so a stray debug print cannot
--- corrupt them.
function M.print(...)
  local parts = {}
  for i = 1, select("#", ...) do
    parts[i] = tostring((select(i, ...)))
  end
  log("print", table.concat(parts, "\t"))
end

-- ---------------------------------------------------------------------------
-- qd.schema
--
-- A validator is anything callable as `(value, path) -> value` that raises on
-- failure, so a plain function is a valid one. The combinators below build
-- them, carrying two hints used for error messages: `kind` is the Lua type
-- they accept (nil when they accept several) and `desc` names them in prose.
-- ---------------------------------------------------------------------------

local VALIDATOR = {
  __call = function(self, value, path)
    return self.run(value, path or "value")
  end,
}

local function validator(kind, desc, run)
  return setmetatable({ kind = kind, desc = desc, run = run }, VALIDATOR)
end

local function is_validator(x)
  return type(x) == "function" or (type(x) == "table" and getmetatable(x) == VALIDATOR)
end

--- Read a hint off a validator; plain functions carry none.
local function hint(spec, field, default)
  if type(spec) == "table" then
    return spec[field] or default
  end
  return default
end

local function expect_validator(spec, who)
  if not is_validator(spec) then
    fail("%s: expected a validator from qd.schema, got %s", who, type(spec))
  end
  return spec
end

-- A table counts as a list when every key is an integer 1..n.
local function is_list(t)
  local n = 0
  for k in pairs(t) do
    if type(k) ~= "number" or k % 1 ~= 0 or k < 1 then
      return false
    end
    n = n + 1
  end
  return n == #t
end

--- "a" / "a or b" / "a, b or c"
local function join(descs)
  if #descs <= 1 then
    return descs[1] or "valid"
  end
  local head = table.concat(descs, ", ", 1, #descs - 1)
  return head .. " or " .. descs[#descs]
end

local S = {}

local function scalar(name, article)
  return function()
    return validator(name, article .. " " .. name, function(value, path)
      if type(value) ~= name then
        fail("`%s` must be %s %s, got %s", path, article, name, type(value))
      end
      return value
    end)
  end
end

S.string = scalar("string", "a")
S.number = scalar("number", "a")
S.boolean = scalar("boolean", "a")

--- Accepts anything, nil included.
function S.any()
  return validator(nil, "anything", function(value)
    return value
  end)
end

--- A list whose every element satisfies `item`.
function S.list(item)
  expect_validator(item, "qd.schema.list")
  return validator("table", "a list of " .. hint(item, "desc", "values"), function(value, path)
    if type(value) ~= "table" or not is_list(value) then
      fail("`%s` must be a list, got %s", path, type(value))
    end
    for i, v in ipairs(value) do
      item(v, ("%s[%d]"):format(path, i))
    end
    return value
  end)
end

--- A table with exactly these fields. Unknown keys are an error; use
--- `optional` for fields that may be absent.
function S.table(fields)
  if type(fields) ~= "table" then
    fail("qd.schema.table: expected a table of fields, got %s", type(fields))
  end
  for name, spec in pairs(fields) do
    expect_validator(spec, ("qd.schema.table field `%s`"):format(tostring(name)))
  end
  return validator("table", "a table", function(value, path)
    if type(value) ~= "table" then
      fail("`%s` must be a table, got %s", path, type(value))
    end
    for k in pairs(value) do
      if fields[k] == nil then
        fail("unknown field `%s.%s`", path, tostring(k))
      end
    end
    for k, spec in pairs(fields) do
      spec(value[k], ("%s.%s"):format(path, k))
    end
    return value
  end)
end

S.record = S.table

--- A table with arbitrary string keys whose values satisfy `item`.
function S.map(item)
  expect_validator(item, "qd.schema.map")
  return validator("table", "a table of " .. hint(item, "desc", "values"), function(value, path)
    if type(value) ~= "table" then
      fail("`%s` must be a table, got %s", path, type(value))
    end
    for k, v in pairs(value) do
      if type(k) ~= "string" then
        fail("`%s` keys must be strings, got %s", path, type(k))
      end
      item(v, ("%s.%s"):format(path, k))
    end
    return value
  end)
end

--- `inner`, or absent.
function S.optional(inner)
  expect_validator(inner, "qd.schema.optional")
  return validator(hint(inner, "kind"), hint(inner, "desc", "valid"), function(value, path)
    if value == nil then
      return nil
    end
    return inner(value, path)
  end)
end

--- One of a fixed set of values.
function S.enum(...)
  local allowed = { ... }
  local shown = {}
  for i, a in ipairs(allowed) do
    shown[i] = ("`%s`"):format(tostring(a))
  end
  local desc = join(shown)
  return validator(nil, desc, function(value, path)
    for _, a in ipairs(allowed) do
      if value == a then
        return value
      end
    end
    fail("`%s` must be %s, got `%s`", path, desc, tostring(value))
  end)
end

--- A union. When exactly one alternative accepts the value's type its error is
--- surfaced directly, so a typo in a record still names the offending field
--- rather than reporting that nothing matched.
function S.one_of(...)
  local alts = { ... }
  if #alts < 2 then
    fail("qd.schema.one_of: needs at least two alternatives")
  end
  local descs = {}
  for i, a in ipairs(alts) do
    expect_validator(a, "qd.schema.one_of")
    descs[i] = hint(a, "desc", "valid")
  end
  local desc = join(descs)
  return validator(nil, desc, function(value, path)
    local candidates = {}
    for _, a in ipairs(alts) do
      local kind = hint(a, "kind")
      if kind == nil or kind == type(value) then
        candidates[#candidates + 1] = a
      end
    end
    if #candidates == 1 then
      return candidates[1](value, path)
    end
    for _, a in ipairs(candidates) do
      local ok, result = pcall(a, value, path)
      if ok then
        return result
      end
    end
    fail("`%s` must be %s, got %s", path, desc, type(value))
  end)
end

M.schema = S

--- Validate `value` against `spec`, naming `path` in any error, and return it.
function M.check(value, spec, path)
  expect_validator(spec, "qd.check")
  return spec(value, path or "value")
end

return M
