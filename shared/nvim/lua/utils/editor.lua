local M = {}

---@param editor EditorConfig
---@param ui UiConfig
function M.setup(editor, ui)
  for name, value in pairs(editor.globals) do
    vim.g[name] = value
  end
  vim.g.have_nerd_font = ui.nerd_font
  for name, value in pairs(vim.tbl_extend("force", editor.options, ui.options)) do
    vim.opt[name] = value
  end
  for name, command in pairs(editor.commands) do
    vim.api.nvim_create_user_command(name, command.action, { desc = command.desc })
  end
  for name, autocmd in pairs(editor.autocmds) do
    vim.api.nvim_create_autocmd(autocmd.events, {
      group = vim.api.nvim_create_augroup(name, { clear = true }),
      desc = autocmd.desc,
      command = autocmd.command,
      callback = autocmd.callback,
    })
  end
  vim.diagnostic.config(ui.diagnostics)
end

---@class BufferMappingOptions: vim.keymap.set.Opts
---@field method? vim.lsp.protocol.Method.ClientToServer

---@alias BufferMapping lib.Mapping<BufferMappingOptions>

---@param bufnr integer
---@param mappings BufferMapping[]
---@param client? vim.lsp.Client
function M.buffer_maps(bufnr, mappings, client)
  for _, mapping in ipairs(mappings) do
    local method = mapping.opts.method
    if not method or (client and client:supports_method(method, bufnr)) then
      local opts = vim.tbl_extend("force", mapping.opts, { buffer = bufnr })
      opts.method = nil
      vim.keymap.set(mapping.mode, mapping.lhs, mapping.rhs, opts)
    end
  end
end

---@param mappings BufferMapping[]
---@return fun(bufnr: integer)
function M.on_attach(mappings)
  return function(bufnr)
    M.buffer_maps(bufnr, mappings)
  end
end

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
function M.shared_config(name)
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
function M.project_config(bufnr, markers)
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
function M.fallback_config(bufnr, markers, name)
  if M.project_config(bufnr, markers) then
    return nil
  end
  return M.shared_config(name)
end

---@param bufnr integer
---@param markers string[]
---@param name string file name inside `shared/tools/`
---@return string|nil
function M.resolve_config(bufnr, markers, name)
  return M.project_config(bufnr, markers) or M.shared_config(name)
end

---@param fraction number
---@return fun(): integer
function M.columns(fraction)
  return function()
    return math.floor(vim.o.columns * fraction)
  end
end

---@param fraction number
---@return fun(): integer
function M.lines(fraction)
  return function()
    return math.floor(vim.o.lines * fraction)
  end
end

---@param size { horizontal: integer, vertical: number }
---@return fun(term: Terminal): number?
function M.terminal_size(size)
  return function(term)
    if term.direction == "horizontal" then
      return size.horizontal
    elseif term.direction == "vertical" then
      return vim.o.columns * size.vertical
    end
  end
end

---@return boolean
function M.has_make()
  return vim.fn.executable "make" == 1
end

---@return boolean
function M.is_windows()
  return vim.fn.has "win32" == 1
end

---@param command string
---@return string?
function M.make_build(command)
  if not M.is_windows() and M.has_make() then
    return command
  end
end

---@param spec LazySpec
---@param opts LazyConfig
---@param source { repository: string, branch: string }
function M.plugins(spec, opts, source)
  local path = vim.fn.stdpath "data" .. "/lazy/lazy.nvim"
  if not vim.uv.fs_stat(path) then
    local output = vim.fn.system {
      "git",
      "clone",
      "--filter=blob:none",
      "--branch=" .. source.branch,
      source.repository,
      path,
    }
    if vim.v.shell_error ~= 0 then
      error("Error cloning lazy.nvim:\n" .. output)
    end
  end

  vim.opt.rtp:prepend(path)
  require("lazy").setup(spec, opts)
end

return M
