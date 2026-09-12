local window = require "utils.editor"

local nerd_font = true

local border = "curved"

---@class UiConfig
local M = {
  nerd_font = nerd_font,

  options = {
    number = true,
    relativenumber = false,
    showmode = false,
    breakindent = true,
    signcolumn = "yes",
    list = true,
    cursorline = true,
    listchars = { tab = "» ", trail = "|", nbsp = "␣" },
  },

  completion = { nerd_font_variant = "mono" },

  git = {
    add = { text = "+" },
    change = { text = "~" },
    delete = { text = "_" },
    topdelete = { text = "‾" },
    changedelete = { text = "~" },
  },

  diagnostics = {
    update_in_insert = false,
    severity_sort = true,
    float = { border = "rounded", source = "if_many" },
    underline = { severity = { min = vim.diagnostic.severity.WARN } },
    virtual_text = true,
    virtual_lines = false,
    jump = {
      on_jump = vim.diagnostic.open_float,
    },
  },

  debug = {
    icons = { expanded = "▾", collapsed = "▸", current_frame = "*" },
    controls = {
      icons = {
        pause = "⏸",
        play = "▶",
        step_into = "⏎",
        step_over = "⏭",
        step_out = "⏮",
        step_back = "b",
        run_last = "▶▶",
        terminate = "⏹",
        disconnect = "⏏",
      },
    },
  },

  todo = { signs = false },

  statusline = { use_icons = nerd_font, location = "%2l:%-2v" },

  lazy = {
    manager = {
      icons = nerd_font and {} or {
        cmd = "⌘",
        config = "🛠",
        event = "📅",
        ft = "📂",
        init = "⚙",
        keys = "🗝",
        plugin = "🔌",
        runtime = "💻",
        require = "🌙",
        source = "📄",
        start = "🚀",
        task = "📌",
        lazy = "💤 ",
      },
    },
    sync = { show = false },
  },

  ---@class SearchAppearance
  search = {
    prompt_titles = { files = "Files  %s Grep", grep = "Grep  %s Files", open_files = "Live Grep in Open Files" },
    buffer = { winblend = 10, previewer = false },
    select = {},
  },

  terminal = {
    toggleterm = {
      size = window.terminal_size { horizontal = 15, vertical = 0.4 },
      shade_terminals = false,
      direction = "float",
      float_opts = { border = border, width = window.columns(0.85), height = window.lines(0.8), winblend = 0 },
      highlights = { FloatBorder = { link = "FloatBorder" } },
    },
    lazygit = {
      direction = "float",
      float_opts = { border = border, width = window.columns(0.95), height = window.lines(0.9) },
    },
  },
}

return M
