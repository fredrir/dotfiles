local M = {}

---@type SearchAppearance
local appearance
---@type PickerKeys
local keys

---@param ui SearchAppearance
---@param mappings PickerKeys
function M.setup(ui, mappings)
  appearance, keys = ui, mappings
end

---@alias utils.SearchKind "files"|"grep"
---@alias utils.PickerMap fun(modes: string|string[], key: string, action: fun(), opts?: vim.keymap.set.Opts)
---@alias utils.PickerAttach fun(prompt_bufnr: integer, map: utils.PickerMap): boolean

---@class utils.SearchOptions
---@field terminal? boolean
---@field default_text? string
---@field cwd? string
---@field prompt_title? string
---@field attach_mappings? utils.PickerAttach

---@param prompt_bufnr integer
---@param search fun(opts?: utils.SearchOptions)
---@param terminal boolean
---@return fun()
local function switch_to(prompt_bufnr, search, terminal)
  return function()
    local prompt = require("telescope.actions.state").get_current_line()
    require("telescope.actions").close(prompt_bufnr)
    vim.schedule_wrap(search) { default_text = prompt, terminal = terminal }
  end
end

local function quit_neovim()
  vim.cmd "qall!"
end

---@param prompt_bufnr integer
local function attach_terminal(prompt_bufnr)
  local picker = require("telescope.actions.state").get_current_picker(prompt_bufnr)

  local function quit()
    require("telescope.actions").close(prompt_bufnr)
    vim.schedule(quit_neovim)
  end

  for _, bufnr in pairs { picker.prompt_bufnr, picker.results_bufnr, picker.preview_bufnr } do
    if vim.api.nvim_buf_is_valid(bufnr) then
      for _, mapping in ipairs(keys.close_terminal) do
        local opts = vim.tbl_extend("force", keys.terminal_opts, { buffer = bufnr, desc = mapping.desc })
        vim.keymap.set(mapping.mode, mapping.lhs, quit, opts)
      end
    end
  end

  vim.schedule(function()
    if vim.api.nvim_win_is_valid(picker.prompt_win) then
      vim.api.nvim_set_current_win(picker.prompt_win)
    end
  end)
end

---@param terminal boolean
---@return utils.PickerAttach
local function attach_picker(terminal)
  return function(prompt_bufnr, map)
    local searches = { files = M.files, grep = M.grep }
    for kind, mapping in pairs(keys.switch) do
      map(mapping.mode, mapping.lhs, switch_to(prompt_bufnr, searches[kind], terminal), { desc = mapping.desc })
    end

    if terminal then
      attach_terminal(prompt_bufnr)
    end

    return true
  end
end

---@param kind utils.SearchKind
---@param opts? utils.SearchOptions
---@return utils.SearchOptions
local function picker_options(kind, opts)
  local options = vim.tbl_extend("force", {
    prompt_title = appearance.prompt_titles[kind]:format(keys.switch[kind == "files" and "grep" or "files"].lhs),
    attach_mappings = attach_picker(opts ~= nil and opts.terminal == true),
  }, opts or {})
  options.terminal = nil
  return options
end

---@param opts? utils.SearchOptions
function M.files(opts)
  require("telescope.builtin").find_files(picker_options("files", opts))
end

---@param opts? utils.SearchOptions
function M.grep(opts)
  require("telescope.builtin").live_grep(picker_options("grep", opts))
end

function M.terminal()
  vim.schedule_wrap(M.files) { terminal = true }
end

function M.buffer()
  require("telescope.builtin").current_buffer_fuzzy_find(require("telescope.themes").get_dropdown(appearance.buffer))
end

function M.open_files()
  require("telescope.builtin").live_grep {
    grep_open_files = true,
    prompt_title = appearance.prompt_titles.open_files,
  }
end

function M.neovim()
  require("telescope.builtin").find_files { cwd = vim.fn.stdpath "config" }
end

return M
