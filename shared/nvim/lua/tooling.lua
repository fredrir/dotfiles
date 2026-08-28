--- Where the command-line tooling keeps its configuration, and how to point a
--- formatter or linter in the editor at the very same files.
---
--- `dotfile format` runs ruff, biome, stylua, taplo, yamlfmt, yamllint,
--- sqlfluff and shfmt against the copies in `shared/tools/`. conform and
--- nvim-lint have to reach those or the editor and the CLI quietly disagree,
--- and several of these tools only ever search upward from the file -- so a
--- config kept outside the tree is never found and the tool runs at its own
--- defaults instead. taplo is the worst of them: it does not read
--- `~/.config/taplo/taplo.toml` at all, so every `.toml` formatted here used
--- taplo's defaults rather than this repo's `column_width` and `align_entries`.
---
--- A project that ships its own config still wins. `M.fallback` injects only
--- when the buffer has nothing of its own anywhere above it.
local M = {}

local home = vim.uv.os_homedir()

--- Memoized `shared/tools/`: a path once found, `false` once we know this
--- machine has no checkout to read it from.
local dir

--- `shared/tools/`, found through the symlink that makes this repo's
--- `shared/nvim` the runtime config directory, with the usual clone location
--- as a fallback for a machine that links `~/.config/nvim` some other way.
local function tools_dir()
  if dir == nil then
    dir = false
    local candidates = {}
    local config = vim.uv.fs_realpath(vim.fn.stdpath "config")
    if config then
      table.insert(candidates, vim.fs.joinpath(vim.fs.dirname(config), "tools"))
    end
    if home then
      table.insert(candidates, vim.fs.joinpath(home, "dotfiles", "shared", "tools"))
    end
    for _, candidate in ipairs(candidates) do
      if vim.uv.fs_stat(candidate) then
        dir = candidate
        break
      end
    end
  end
  return dir or nil
end

--- Absolute path of a `shared/tools/` config, or nil when it is not there.
---@param name string file name inside `shared/tools/`
---@return string|nil
function M.config(name)
  local d = tools_dir()
  if not d then
    return nil
  end
  local path = vim.fs.joinpath(d, name)
  return vim.uv.fs_stat(path) and path or nil
end

--- The nearest config above the buffer that a project owns, or nil.
---
--- The upward walk stops below `$HOME` on purpose: the source-of-truth configs
--- are symlinked into the home directory, and finding one of those would look
--- like a project config when it is the thing being injected.
---@param bufnr integer
---@param markers string[] config file names that mean "this project owns it"
---@return string|nil
function M.nearest(bufnr, markers)
  local file = vim.api.nvim_buf_get_name(bufnr)
  if file == "" then
    return nil
  end
  return vim.fs.find(markers, { path = file, upward = true, type = "file", limit = 1, stop = home })[1]
end

--- The `shared/tools/` config, unless the buffer sits inside a project carrying
--- one of `markers` -- in which case that project's config governs and nothing
--- may be injected over it. For tools that resolve a project config correctly
--- on their own: taplo and biome, which both search upward from the file.
---@param bufnr integer
---@param markers string[]
---@param name string file name inside `shared/tools/`
---@return string|nil
function M.fallback(bufnr, markers, name)
  if M.nearest(bufnr, markers) then
    return nil
  end
  return M.config(name)
end

--- The project's own config if it has one, and the `shared/tools/` copy
--- otherwise. For tools that cannot find a project config from the buffer at
--- all -- yamllint searches the *working directory*, which in an editor has
--- nothing to do with the file being linted.
---@param bufnr integer
---@param markers string[]
---@param name string file name inside `shared/tools/`
---@return string|nil
function M.resolve(bufnr, markers, name)
  return M.nearest(bufnr, markers) or M.config(name)
end

return M
