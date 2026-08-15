local platform = require 'wez.platform'
local status = require 'wez.appearance.status'
local tabs = require 'wez.appearance.tabs'
local theme = require 'wez.theme'
local wezterm = require 'wezterm'

local M = {}

function M.apply(config)
  config.colors = {
    foreground = theme.colors.foreground,
    background = theme.colors.background,
    cursor_bg = theme.colors.cursor_bg,
    cursor_fg = theme.colors.cursor_fg,
    cursor_border = theme.colors.cursor_border,
    selection_fg = theme.colors.selection_fg,
    selection_bg = theme.colors.selection_bg,
    ansi = theme.colors.ansi,
    brights = theme.colors.brights,
  }

  config.font = wezterm.font(theme.fonts.nerd)
  config.font_size = platform.pick {
    mac = theme.sizes.terminal_mac,
    default = theme.sizes.terminal,
  }
  config.adjust_window_size_when_changing_font_size = false

  config.window_padding = {
    left = 8,
    right = 8,
    top = 8,
    bottom = 8,
  }
  config.window_background_opacity = 1.0
  config.window_close_confirmation = 'NeverPrompt'
  config.default_cursor_style = 'SteadyBlock'
  config.scrollback_lines = 100000
  config.audible_bell = 'Disabled'

  config.enable_wayland = true

  -- The Norwegian layout puts ~ ` ´ ^ on dead keys and | [ ] \ { } behind
  -- AltGr, so composing must be left alone or those characters are
  -- unreachable.
  config.use_dead_keys = false
  if platform.is_mac then
    config.send_composed_key_when_left_alt_is_pressed = true
    config.send_composed_key_when_right_alt_is_pressed = true
  end

  tabs.apply(config)
end

function M.setup()
  tabs.setup()
  status.setup()
end

return M
