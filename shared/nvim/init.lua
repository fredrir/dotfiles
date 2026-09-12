local root = vim.fn.stdpath "config"
---@type EditorConfig
local editor = dofile(root .. "/editor.lua")
local ui = require "ui"
local runtime = require "utils.editor"

runtime.setup(editor, ui)
require("utils.clipboard").setup()
vim.filetype.add { extension = require("languages").extensions() }

---@type KeymapConfig
local keys = dofile(root .. "/keymap.lua")
require("utils.session").setup(ui.lazy.sync)
require("utils.search").setup(ui.search, keys.telescope)
require("utils.actions").setup {
  lazygit = vim.tbl_deep_extend("force", editor.git.lazygit, ui.terminal.lazygit),
  shell_filetypes = require("languages").filetypes "shell",
  shell_show = editor.completion.shell_show,
  minimum_keyword_length = editor.completion.minimum_keyword_length,
}

---@type fun(editor: EditorConfig, ui: UiConfig, keys: KeymapConfig): LazySpec
local plugins = dofile(root .. "/plugins.lua")
runtime.plugins(
  plugins(editor, ui, keys),
  { rocks = editor.plugins.rocks, ui = ui.lazy.manager },
  editor.plugins.source
)
