vim.g.mapleader = " "
vim.g.maplocalleader = " "
vim.g.have_nerd_font = true

do
  local m = vim.env.NVIM_MINIMAL
  vim.g.minimal = m ~= nil and m ~= "" and m ~= "0"
end

-- [[ Options ]]
vim.o.number = true
vim.o.relativenumber = false
vim.o.mouse = "a"
vim.o.showmode = false
local is_ssh = vim.env.SSH_CONNECTION ~= nil or vim.env.SSH_TTY ~= nil
local has_native_clipboard = vim.fn.has "mac" == 1 or vim.env.WAYLAND_DISPLAY ~= nil or vim.env.DISPLAY ~= nil

if is_ssh or not has_native_clipboard then
  local osc52 = require "vim.ui.clipboard.osc52"
  local last_copy_file = vim.fn.stdpath "cache" .. "/osc52-clipboard"

  local function copy(register)
    local send = osc52.copy(register)
    return function(lines)
      pcall(vim.fn.writefile, lines, last_copy_file)
      send(lines)
    end
  end

  local function paste_last_copy()
    if vim.fn.filereadable(last_copy_file) == 0 then
      return {}
    end
    return vim.fn.readfile(last_copy_file)
  end

  vim.g.clipboard = {
    name = "osc52",
    copy = { ["+"] = copy "+", ["*"] = copy "*" },
    paste = { ["+"] = paste_last_copy, ["*"] = paste_last_copy },
    cache_enabled = true,
  }
end
vim.o.clipboard = "unnamedplus"
vim.o.breakindent = true
vim.o.undofile = true
vim.o.ignorecase = true
vim.o.smartcase = true
vim.o.signcolumn = "yes"
vim.o.updatetime = 250
vim.o.timeoutlen = 300
vim.o.splitright = true
vim.o.splitbelow = true
vim.o.list = true
vim.opt.listchars = { tab = "» ", trail = "|", nbsp = "␣" }
vim.o.inccommand = "split"
vim.o.cursorline = true
vim.o.scrolloff = 10
vim.o.confirm = true
vim.o.autoread = true

vim.api.nvim_create_autocmd({ "FocusGained", "BufEnter", "CursorHold" }, {
  command = "checktime",
})

-- [[ Keymaps ]]
vim.keymap.set("n", "<Esc>", "<cmd>nohlsearch<CR>")

vim.diagnostic.config {
  update_in_insert = false,
  severity_sort = true,
  float = { border = "rounded", source = "if_many" },
  underline = { severity = { min = vim.diagnostic.severity.WARN } },
  virtual_text = true,
  virtual_lines = false,
  jump = {
    on_jump = vim.diagnostic.open_float,
  },
}

vim.keymap.set("n", "<leader>q", vim.diagnostic.setloclist, { desc = "Open diagnostic [Q]uickfix list" })
vim.keymap.set("t", "<Esc><Esc>", "<C-\\><C-n>", { desc = "Exit terminal mode" })

vim.keymap.set("v", "J", ":m '>+1<CR>gv=gv", { desc = "Move selection down" })
vim.keymap.set("v", "K", ":m '<-2<CR>gv=gv", { desc = "Move selection up" })

vim.keymap.set("n", "<S-h>", "<cmd>bprevious<CR>", { desc = "Previous buffer" })
vim.keymap.set("n", "<S-l>", "<cmd>bnext<CR>", { desc = "Next buffer" })
vim.keymap.set("n", "<leader>x", "<cmd>bdelete<CR>", { desc = "Close buffer" })

vim.keymap.set("n", "<leader>p", '"_dP', { desc = "Replace line with yanked content" })

vim.keymap.set("n", "<leader>e", "<cmd>Neotree toggle<CR>", { desc = "File [E]xplorer" })

vim.keymap.set("n", "<C-h>", "<C-w><C-h>", { desc = "Move focus to the left window" })
vim.keymap.set("n", "<C-l>", "<C-w><C-l>", { desc = "Move focus to the right window" })
vim.keymap.set("n", "<C-j>", "<C-w><C-j>", { desc = "Move focus to the lower window" })
vim.keymap.set("n", "<C-k>", "<C-w><C-k>", { desc = "Move focus to the upper window" })

-- Typos
vim.api.nvim_create_user_command("W", "w", {})
vim.api.nvim_create_user_command("Q", "q", {})
vim.api.nvim_create_user_command("WQ", "wq", {})
vim.api.nvim_create_user_command("Wq", "wq", {})

-- [[ Autocommands ]]
vim.api.nvim_create_autocmd("TextYankPost", {
  desc = "Highlight yanks",
  group = vim.api.nvim_create_augroup("highlight-yank", { clear = true }),
  callback = function()
    vim.hl.on_yank()
  end,
})

-- [[ Lazy.nvim ]]
local lazypath = vim.fn.stdpath "data" .. "/lazy/lazy.nvim"
if not (vim.uv or vim.loop).fs_stat(lazypath) then
  local lazyrepo = "https://github.com/folke/lazy.nvim.git"
  local out = vim.fn.system { "git", "clone", "--filter=blob:none", "--branch=stable", lazyrepo, lazypath }
  if vim.v.shell_error ~= 0 then
    error("Error cloning lazy.nvim:\n" .. out)
  end
end

---@type vim.Option
local rtp = vim.opt.rtp
rtp:prepend(lazypath)

vim.g.python3_host_prog = vim.fn.expand "~/.local/share/nvim/pynvim-venv/bin/python"
vim.g.loaded_node_provider = 0

require("lazy").setup({
  { "NMAC427/guess-indent.nvim", opts = {} },
  { import = "plugins" },
}, {
  rocks = { enabled = false },
  ui = {
    icons = vim.g.have_nerd_font and {} or {
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
})
