local M = {}

---@alias utils.SearchKind "files"|"grep"

---@class utils.SearchOptions
---@field default_text? string
---@field cwd? string
---@field terminal? boolean

local open

local titles = { files = "Files  <C-g> Grep", grep = "Grep  <C-f> Files" }
local pickers = { files = "find_files", grep = "live_grep" }

---@param prompt_bufnr integer
---@param kind utils.SearchKind
---@param opts utils.SearchOptions
local function switch_to(prompt_bufnr, kind, opts)
  return function()
    local prompt = require("telescope.actions.state").get_current_line()
    require("telescope.actions").close(prompt_bufnr)
    vim.schedule(function()
      open(kind, { default_text = prompt, cwd = opts.cwd, terminal = opts.terminal })
    end)
  end
end

---@return integer[]
local function picker_buffers(picker)
  local buffers = {}
  for _, bufnr in pairs { picker.prompt_bufnr, picker.results_bufnr, picker.preview_bufnr } do
    if vim.api.nvim_buf_is_valid(bufnr) then
      buffers[#buffers + 1] = bufnr
    end
  end
  return buffers
end

local function bind_quit(picker)
  local function quit()
    require("telescope.actions").close(picker.prompt_bufnr)
    vim.schedule(function()
      vim.cmd "qall!"
    end)
  end

  for _, bufnr in ipairs(picker_buffers(picker)) do
    vim.keymap.set({ "i", "n" }, "<Esc>", quit, { nowait = true, buffer = bufnr, desc = "Close terminal search" })
    vim.keymap.set("i", "<C-c>", quit, { nowait = true, buffer = bufnr, desc = "Close terminal search" })
    vim.keymap.set("n", "q", quit, { nowait = true, buffer = bufnr, desc = "Close terminal search" })
  end

  vim.schedule(function()
    if vim.api.nvim_win_is_valid(picker.prompt_win) then
      vim.api.nvim_set_current_win(picker.prompt_win)
    end
  end)
end

---@param kind utils.SearchKind
---@param prompt_bufnr integer
---@param opts utils.SearchOptions
local function attach(kind, prompt_bufnr, picker_map, opts)
  if kind == "files" then
    picker_map({ "i", "n" }, "<C-g>", switch_to(prompt_bufnr, "grep", opts), { desc = "Search by grep" })
  else
    picker_map({ "i", "n" }, "<C-f>", switch_to(prompt_bufnr, "files", opts), { desc = "Search files" })
  end

  if opts.terminal then
    bind_quit(require("telescope.actions.state").get_current_picker(prompt_bufnr))
  end

  return true
end

---@param kind utils.SearchKind
---@param opts? utils.SearchOptions
open = function(kind, opts)
  opts = opts or {}
  require("telescope.builtin")[pickers[kind]] {
    prompt_title = titles[kind],
    default_text = opts.default_text,
    cwd = opts.cwd,
    attach_mappings = function(prompt_bufnr, picker_map)
      return attach(kind, prompt_bufnr, picker_map, opts)
    end,
  }
end

function M.files()
  open "files"
end

function M.grep()
  open "grep"
end

function M.terminal()
  vim.schedule(function()
    open("files", { terminal = true })
  end)
end

function M.neovim()
  open("files", { cwd = vim.fn.stdpath "config" })
end

function M.buffer()
  local themes = require "telescope.themes"
  require("telescope.builtin").current_buffer_fuzzy_find(themes.get_dropdown { winblend = 10, previewer = false })
end

function M.open_files()
  require("telescope.builtin").live_grep { grep_open_files = true, prompt_title = "Live Grep in Open Files" }
end

return M
