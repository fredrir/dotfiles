local wezterm = require "wezterm"

local M = {}

local dir = wezterm.home_dir .. "/dotfiles"

M.dir = dir
M.bin_dir = dir .. "/scripts/shell"
M.compiled_dir = dir .. "/.bin"

return M
