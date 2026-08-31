-- Generated from theme/profiles/sexy-purple.toml
local wezterm = require "wezterm"
local platform = require "utils.platform"

local terminal = require "ui.fonts.nerd"
local interface = require "ui.fonts.general"

local M = {}

function M.apply_to_config(config)
  config.font = wezterm.font_with_fallback { terminal.family }
  config.font_size = platform.is_mac and 13 or 12

  config.window_frame = config.window_frame or {}
  config.window_frame.font = wezterm.font_with_fallback { interface.family }
  config.window_frame.font_size = 10
end

return M
