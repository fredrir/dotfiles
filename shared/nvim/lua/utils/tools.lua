local M = {}

local home = vim.uv.os_homedir()
local cached_dir

---@return string?
local function shared_dir()
  if cached_dir == nil then
    cached_dir = false
    local config = vim.uv.fs_realpath(vim.fn.stdpath "config")
    local candidates = {}
    if config then
      candidates[#candidates + 1] = vim.fs.joinpath(vim.fs.dirname(config), "tools")
    end
    if home then
      candidates[#candidates + 1] = vim.fs.joinpath(home, "dotfiles", "shared", "tools")
    end
    for _, candidate in ipairs(candidates) do
      if vim.uv.fs_stat(candidate) then
        cached_dir = candidate
        break
      end
    end
  end
  return cached_dir or nil
end

---@param name string
---@return string?
function M.shared(name)
  local dir = shared_dir()
  if not dir then
    return nil
  end
  local path = vim.fs.joinpath(dir, name)
  return vim.uv.fs_stat(path) and path or nil
end

---@param bufnr integer
---@param markers string[]
---@return string?
function M.project(bufnr, markers)
  local file = vim.api.nvim_buf_get_name(bufnr)
  if file == "" then
    return nil
  end
  return vim.fs.find(markers, { path = file, upward = true, type = "file", limit = 1, stop = home })[1]
end

return M
