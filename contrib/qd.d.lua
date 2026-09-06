---@meta
-- Type stubs for `qd.lua` files. Point lua-language-server at this file
-- (workspace.library) for completion and checks.

---@class qd.host
---@field name "darwin"|"linux"|"windows"
---@field darwin boolean
---@field linux boolean
---@field windows boolean
---@field posix boolean
---@field wsl boolean
---@field ubuntu boolean
---@field distro string|nil  # `ID` from /etc/os-release on Linux

---@class qd.path
---@field home fun(...: string): string
---@field config fun(...: string): string        # ~/.config
---@field cache fun(...: string): string         # ~/.dotfiles-cache
---@field app_support fun(...: string): string   # ~/Library/Application Support
---@field appdata fun(...: string): string       # %APPDATA% (Windows only)
---@field local_appdata fun(...: string): string # %LOCALAPPDATA% (Windows only)
---@field dotfiles fun(...: string): string      # repo root
---@field join fun(...: string): string
---@field is_absolute fun(path: string): boolean

---@class qd
---@field host qd.host
---@field path qd.path
---@field tag fun(name: string): boolean            # `qd tag add` or $DOTFILES_TAGS; declared, never detected
---@field list fun(base: any[], ...: any): any[]    # copy `base` and append
---@field env fun(name: string): string|nil
---@field exists fun(path: string): boolean
---@field which fun(cmd: string): string|nil        # first match on PATH
---@field fail fun(fmt: string, ...: any)            # raise a config error, no file:line prefix
---@field warn fun(fmt: string, ...: any)            # to stderr, always shown
---@field debug fun(fmt: string, ...: any)           # to stderr, only when QD_DEBUG is set
---@field check fun(value: any, spec: qd.Validator, path?: string): any
---@field schema qd.schema                           # validator combinators
---@field run fun(cmd: string, ...: string)          # hooks only; errors on non-zero exit
---@field exec fun(cmd: string, ...: string): string # hooks only; returns stdout
---@field write fun(path: string, content: string)   # hooks only
qd = {}

---Anything callable as `(value, path) -> value` that raises on failure, so a
---plain function is a valid validator:
---
---```lua
---local even = function(v, path)
---  if v % 2 ~= 0 then qd.fail("`%s` must be even", path) end
---  return v
---end
---```
---@alias qd.Validator fun(value: any, path: string): any

---Combinators that build validators. Reach them through the namespace:
---`qd.schema.table` and `qd.schema.string` would shadow the Lua stdlib
---modules of those names if pulled into locals.
---
---```lua
---local s = qd.schema
---qd.check(cfg, s.table {
---  source     = s.optional(s.list(s.string())),
---  env_source = s.optional(s.list(s.string())),
---}, "nushell")
---```
---@class qd.schema
---@field string fun(): qd.Validator
---@field number fun(): qd.Validator
---@field boolean fun(): qd.Validator
---@field any fun(): qd.Validator                                    # anything, nil included
---@field list fun(item: qd.Validator): qd.Validator
---@field table fun(fields: table<string, qd.Validator>): qd.Validator  # exact fields
---@field record fun(fields: table<string, qd.Validator>): qd.Validator # alias of `table`
---@field map fun(item: qd.Validator): qd.Validator                  # any string keys
---@field optional fun(inner: qd.Validator): qd.Validator
---@field enum fun(...: any): qd.Validator                           # one of these values
---@field one_of fun(...: qd.Validator): qd.Validator                # a union

---`print` is redirected to stderr: stdout carries `qd show --format json` and
---the shell completion lists, which a stray print would corrupt.
---@param ... any
function print(...) end

---@class qd.FilePair
---@field src string      # relative to the module directory in the repo
---@field dest string     # absolute
---@field enabled boolean|nil

---@class qd.Setup
---@field version integer|nil                 # bump to rerun hooks on every machine
---@field before fun(m: qd.ModuleView)|nil    # before the first apply
---@field after fun(m: qd.ModuleView)|nil     # after the first apply

-- Core fields. Any other key must be the name of a loaded plugin.
---@class qd.Module
---@field enabled boolean|nil
---@field path string|nil                     # destination directory, absolute
---@field files qd.FilePair[]|nil
---@field include string[]|nil                # absolute files copied into `path` on push
---@field ignore string[]|nil                 # globs relative to `path`
---@field encrypt string[]|nil                # globs relative to `path`; stored as <name>.age
---@field setup qd.Setup|nil
---@field brew (string|qd.brew.Package)[]|nil   # built-in plugin
---@field scoop (string|qd.scoop.Package)[]|nil # built-in plugin
---@field nushell qd.nushell.Config|nil         # built-in plugin
---@field [string] any                          # other plugin keys

-- The root qd.lua: shared globs, includes, plugin keys and the plugin list.
---@class qd.Root
---@field plugins qd.Plugin[]|nil             # default: qd.nushell, qd.brew, qd.scoop
---@field include string[]|nil
---@field ignore string[]|nil
---@field encrypt string[]|nil
---@field nushell qd.nushell.Config|nil
---@field [string] any

-- What hooks and plugins see for a module (`qd show`): core fields resolved
-- to absolute paths, plugin keys after the plugin's `resolve`.
---@class qd.ModuleView
---@field name string
---@field src string
---@field dest string|nil
---@field enabled boolean
---@field ignore string[]
---@field encrypt string[]
---@field files { src: string, dest: string }[]
---@field include string[]
---@field setup { version: integer, before: boolean, after: boolean }|nil
---@field [string] any

---@class qd.RootView
---@field ignore string[]
---@field encrypt string[]
---@field include string[]
---@field [string] any

-- Passed to `compile` and `packages.*`.
---@class qd.Ctx
---@field root string                # repo root
---@field global qd.RootView
---@field modules qd.ModuleView[]    # enabled modules, sorted by name

-- Passed to `resolve`: the file the key came from.
---@class qd.Target
---@field name string                # module name, or "root"
---@field src string
---@field dest string|nil
---@field root boolean

---@class qd.Output
---@field path string                # absolute
---@field content string

---@class qd.Packages
---@field list fun(ctx: qd.Ctx): any                  # printed by `qd packages list`
---@field install fun(ctx: qd.Ctx): string[][]|nil    # argv lists, run in order
---@field upgrade fun(ctx: qd.Ctx): string[][]|nil

-- A plugin owns the key `name` in every qd.lua. Every function is pure: the
-- core writes the outputs and runs the commands.
---@class qd.Plugin
---@field name string
---@field available fun(): boolean|nil                        # package manager present on this host
---@field resolve fun(m: qd.Target, value: any): any|nil      # validate/normalise the key at load
---@field compile fun(ctx: qd.Ctx): qd.Output[]|nil
---@field packages qd.Packages|nil

---@class qd.brew.Package
---@field name string
---@field tap string|nil

---@class qd.scoop.Package
---@field name string
---@field bucket string|nil   # default "main"

---@class qd.nushell.Config
---@field include string[]|nil      # `use` lines in ~/.dotfiles.local.nu
---@field source string[]|nil       # `source` lines
---@field env_include string[]|nil  # `use` lines in ~/.dotfiles-env.local.nu
---@field env_source string[]|nil   # `source` lines

---`require("qd")` is the API, `require("qd.nushell" | "qd.brew" | "qd.scoop")`
---a built-in plugin, anything else `<repo>/<name>.lua` with dots as separators.
---@param name string
---@return any
function require(name) end
