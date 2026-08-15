local design = require 'wez.design'
local platform = require 'wez.platform'
local status = require 'wez.appearance.status'
local tabs = require 'wez.appearance.tabs'
local wezterm = require 'wezterm'

local M = {}

function M.apply(config)
  local palette = design.palette

  config.colors = {
    foreground = design.colors.foreground,
    background = design.colors.background,
    cursor_bg = design.colors.cursor_bg,
    cursor_fg = design.colors.cursor_fg,
    cursor_border = design.colors.cursor_border,
    selection_fg = design.colors.selection_fg,
    selection_bg = design.colors.selection_bg,
    ansi = design.colors.ansi,
    brights = design.colors.brights,
    indexed = design.colors.indexed,

    split = palette.surface0,
    compose_cursor = palette.peach,
    quick_select_label_bg = { Color = palette.peach },
    quick_select_label_fg = { Color = palette.crust },
    quick_select_match_bg = { Color = palette.surface1 },
    quick_select_match_fg = { Color = palette.text },
    copy_mode_active_highlight_bg = { Color = palette.yellow },
    copy_mode_active_highlight_fg = { Color = palette.crust },
    copy_mode_inactive_highlight_bg = { Color = palette.surface1 },
    copy_mode_inactive_highlight_fg = { Color = palette.text },
  }

  config.command_palette_bg_color = palette.crust
  config.command_palette_fg_color = palette.text
  config.command_palette_font_size = design.sizes.palette
  config.command_palette_rows = 12

  config.font = wezterm.font_with_fallback(design.fonts)
  config.font_size = design.sizes.terminal
  config.line_height = 1.08
  config.harfbuzz_features = { 'calt=1', 'liga=1' }
  config.adjust_window_size_when_changing_font_size = false

  config.window_padding = {
    left = 14,
    right = 14,
    top = 10,
    bottom = 6,
  }
  config.window_decorations = 'RESIZE'
  config.window_background_opacity = platform.pick { mac = 0.95, default = 0.96 }
  config.inactive_pane_hsb = { saturation = 0.92, brightness = 0.82 }
  config.window_close_confirmation = 'NeverPrompt'
  config.default_cursor_style = 'SteadyBlock'
  -- Eagerly allocated per pane (and mirrored by the mux client's line
  -- cache), so 100k was ~14 MB of dead weight per pane before any content.
  config.scrollback_lines = 20000
  config.audible_bell = 'Disabled'

  if platform.is_mac then
    config.macos_window_background_blur = 30
    config.native_macos_fullscreen_mode = false
    config.macos_fullscreen_extend_behind_notch = true
    -- Stay resident so the CMD+§ summon (hammerspoon) is instant even
    -- after the last window closes; CMD+Q remains the real quit.
    config.quit_when_all_windows_are_closed = false
    config.notification_handling = 'SuppressFromFocusedPane'
  end

  if platform.is_linux then
    config.enable_wayland = true
    config.kde_window_background_blur = true
    config.freetype_load_target = 'Light'
    config.freetype_render_target = 'HorizontalLcd'
  end

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
