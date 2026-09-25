require "options"
require "cmds"
require "keymaps"
require("utils.clipboard").setup()

local lazypath = vim.fs.joinpath(vim.fn.stdpath "data", "lazy", "lazy.nvim")

if not vim.uv.fs_stat(lazypath) then
  local output = vim.fn.system {
    "git",
    "clone",
    "--filter=blob:none",
    "--branch=stable",
    "https://github.com/folke/lazy.nvim.git",
    lazypath,
  }
  if vim.v.shell_error ~= 0 then
    error("Error cloning lazy.nvim:\n" .. output)
  end
end

vim.opt.rtp:prepend(lazypath)

require("lazy").setup("plugins", { rocks = { enabled = false } })
