local wezterm = require 'wezterm'
local platform = require 'wez.platform'
local theme = require 'wez.theme'
local tabs = require 'wez.appearance.tabs'

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

  -- WezTerm falls back by codepoint, so this replaces kitty's explicit
  -- symbol_map for the Private Use Area.
  config.font = wezterm.font_with_fallback {
    theme.fonts.nerd,
    'Noto Color Emoji',
  }
  config.font_size = platform.pick { mac = 13.0, default = 12.0 }
  config.adjust_window_size_when_changing_font_size = false

  config.window_padding = { left = 8, right = 8, top = 8, bottom = 8 }
  config.window_background_opacity = 1.0
  config.window_close_confirmation = 'NeverPrompt'
  config.default_cursor_style = 'SteadyBlock'
  config.scrollback_lines = 100000
  config.audible_bell = 'Disabled'

  config.enable_wayland = true

  -- The Norwegian layout puts ~ ` ´ ^ on dead keys and | [ ] \ { } behind
  -- AltGr, so composing must be left alone or those characters are unreachable.
  config.use_dead_keys = false
  if platform.is_mac then
    config.send_composed_key_when_left_alt_is_pressed = true
    config.send_composed_key_when_right_alt_is_pressed = true
  end

  tabs.apply(config)
end

function M.setup()
  tabs.setup()
end

return M
