local M = {}

local home = vim.uv.os_homedir()

local dir

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

---@param bufnr integer
---@param markers string[]
---@return string|nil
function M.nearest(bufnr, markers)
  local file = vim.api.nvim_buf_get_name(bufnr)
  if file == "" then
    return nil
  end
  return vim.fs.find(markers, { path = file, upward = true, type = "file", limit = 1, stop = home })[1]
end

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

---@param bufnr integer
---@param markers string[]
---@param name string file name inside `shared/tools/`
---@return string|nil
function M.resolve(bufnr, markers, name)
  return M.nearest(bufnr, markers) or M.config(name)
end

return M
