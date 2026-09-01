local wezterm = require "wezterm"
local gpu_adapters = require "utils.gpu-adapter"
local performance = require "ui.performance"
local fonts = require "ui.fonts.fonts"
local profiles = require "ui.colors.profiles"

---@type Config
local ui_config = {
  font = wezterm.font_with_fallback { { family = fonts.nerd_family, weight = "Medium" } },
  font_size = fonts.font_size,

  window_frame = {
    font = wezterm.font_with_fallback { { family = fonts.general_family } },
    font_size = fonts.interface_font_size,
  },
  inactive_pane_hsb = {
    hue = 1,
    saturation = 1.0,
    brightness = 1.0,
  },
  colors = profiles.active.colors,
  color_schemes = profiles.profiles,
  max_fps = performance.max_fps,
  webgpu_preferred_adapter = gpu_adapters:pick_best(),
}

return ui_config
