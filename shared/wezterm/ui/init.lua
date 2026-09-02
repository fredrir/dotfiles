local wezterm = require "wezterm"
local performance = require "ui.performance"
local fonts = require "ui.fonts.fonts"
local profiles = require "ui.colors.profiles"

---@type Config
local ui_config = {
  font = wezterm.font_with_fallback { { family = fonts.nerd_family, weight = "Medium" } },
  font_size = fonts.font_size,

  -- tab bar
  hide_tab_bar_if_only_one_tab = false,

  -- window
  window_frame = {
    font = wezterm.font_with_fallback { { family = fonts.general_family } },
    font_size = fonts.interface_font_size,
  },
  window_close_confirmation = "NeverPrompt",
  inactive_pane_hsb = {
    saturation = 1,
    brightness = 1,
  },

  colors = profiles.active.colors,
  color_schemes = profiles.profiles,
  max_fps = performance.max_fps,
  front_end = performance.front_end,
}

return ui_config
