local shell_sources = { "lsp", "path", "snippets", "buffer" }

---@class EditorConfig
local M = {
  globals = {
    mapleader = " ",
    maplocalleader = " ",
    python3_host_prog = vim.env.PYNVIM_PYTHON_PATH,
    loaded_node_provider = 0,
  },
  options = {
    mouse = "a",
    clipboard = "unnamedplus",
    undofile = true,
    ignorecase = true,
    smartcase = true,
    updatetime = 250,
    timeoutlen = 300,
    splitright = true,
    splitbelow = true,
    inccommand = "split",
    scrolloff = 10,
    confirm = true,
    autoread = true,
  },
  ---@class FormatPreferences
  formatting = {
    notify_on_error = true,
    ---@type conform.FormatOpts
    manual = { async = true, lsp_format = "fallback" },
    ---@type conform.FormatOpts
    on_save = { timeout_ms = 500, lsp_format = "fallback" },
    ---@type conform.FormatOpts
    shell = { lsp_format = "prefer", name = "shuck" },
    shell_timeout_ms = 1000,
  },
  completion = {
    documentation = { auto_show = false, auto_show_delay_ms = 250 },
    selection = { preselect = false, auto_insert = false },
    sources = { "lsp", "path", "snippets" },
    sources_by_filetype = {
      sh = shell_sources,
      bash = shell_sources,
      zsh = shell_sources,
      markdown = { "lsp", "path", "buffer" },
    },
    minimum_keyword_length = { default = 0, markdown = 3 },
    shell_show = { initial_selected_item_idx = 1 },
    snippets = { preset = "luasnip" },
    fuzzy = { implementation = "lua" },
    signature = { enabled = true },
  },
  files = { show_hidden = true, clean_session_placeholders = true },
  textobjects = { n_lines = 500 },
  key_hints = { delay = 0 },
  lint = { events = { "BufEnter", "BufWritePost", "InsertLeave" } },
  syntax = { highlight = true, indent = true },
  tools = { mason = { PATH = "append" } },
  debug = { automatic_installation = true, open_ui_on_start = true, close_ui_on_end = true },
  plugins = {
    source = { repository = "https://github.com/folke/lazy.nvim.git", branch = "stable" },
    rocks = { enabled = false },
  },
  commands = {
    W = { action = "w" },
    Q = { action = "q" },
    WQ = { action = "wq" },
    Wq = { action = "wq" },
    NeovimSync = { action = require("utils.session").sync, desc = "Sync Lazy & Restart Neovim" },
    TerminalSearch = { action = require("utils.search").terminal, desc = "Search files from the terminal" },
  },
  autocmds = {
    checktime = { events = { "FocusGained", "BufEnter", "CursorHold" }, command = "checktime" },
    ["highlight-yank"] = { events = { "TextYankPost" }, callback = vim.hl.on_yank, desc = "Highlight yanks" },
  },
  git = { lazygit = { cmd = "lazygit", hidden = true } },
}

return M
