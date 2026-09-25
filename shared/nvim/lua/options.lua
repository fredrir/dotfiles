vim.g.mapleader = " "
vim.g.maplocalleader = " "
vim.g.have_nerd_font = true
vim.g.python3_host_prog = vim.env.PYNVIM_PYTHON_PATH
vim.g.loaded_node_provider = 0

local opt = vim.opt

-- Behavior --

opt.mouse = "a"
opt.clipboard = "unnamedplus"
opt.undofile = true
opt.ignorecase = true
opt.smartcase = true
opt.updatetime = 250
opt.timeoutlen = 300
opt.splitright = true
opt.splitbelow = true
opt.inccommand = "split"
opt.scrolloff = 10
opt.confirm = true
opt.autoread = true

-- Appearance --

opt.number = true
opt.showmode = false
opt.breakindent = true
opt.signcolumn = "yes"
opt.cursorline = true
opt.list = true
opt.listchars = { tab = "» ", trail = "|", nbsp = "␣" }

vim.filetype.add { extension = { config = "dotfile", dotfile = "dotfile" } }

vim.diagnostic.config {
  update_in_insert = false,
  severity_sort = true,
  float = { border = "rounded", source = "if_many" },
  underline = { severity = { min = vim.diagnostic.severity.WARN } },
  virtual_text = true,
  jump = { on_jump = vim.diagnostic.open_float },
}
