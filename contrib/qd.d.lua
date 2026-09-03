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

---@class qd
---@field host qd.host
---@field path qd.path
---@field tag fun(name: string): boolean            # machine tag from `qd tag add`
---@field list fun(base: any[], ...: any): any[]    # copy `base` and append
---@field env fun(name: string): string|nil
---@field exists fun(path: string): boolean
---@field run fun(cmd: string, ...: string)          # hooks only; errors on non-zero exit
---@field exec fun(cmd: string, ...: string): string # hooks only; returns stdout
---@field write fun(path: string, content: string)   # hooks only
qd = {}

---@class qd.Package
---@field name string
---@field tap string|nil     # brew
---@field bucket string|nil  # scoop

---@class qd.FilePair
---@field src string      # relative to the module directory in the repo
---@field dest string     # absolute
---@field enabled boolean|nil

---@class qd.Dotfile
---@field include string[]|nil      # `use` lines in ~/.dotfiles.local.nu
---@field source string[]|nil       # `source` lines
---@field env_include string[]|nil  # `use` lines in ~/.dotfiles-env.local.nu
---@field env_source string[]|nil   # `source` lines

---@class qd.HookArg
---@field name string
---@field src string
---@field dest string|nil
---@field config table

---@class qd.Setup
---@field version integer|nil                 # bump to rerun hooks on every machine
---@field before fun(m: qd.HookArg)|nil       # before the first apply
---@field after fun(m: qd.HookArg)|nil        # after the first apply

---@class qd.Module
---@field enabled boolean|nil
---@field path string|nil                     # destination directory, absolute
---@field brew (string|qd.Package)[]|nil
---@field scoop (string|qd.Package)[]|nil
---@field files qd.FilePair[]|nil
---@field include string[]|nil                # absolute files copied into `path` on push
---@field ignore string[]|nil                 # globs relative to `path`
---@field encrypt string[]|nil                # globs relative to `path`; stored as <name>.age
---@field dotfile qd.Dotfile|nil
---@field setup qd.Setup|nil

---@param name string
---@return any
function require(name) end
