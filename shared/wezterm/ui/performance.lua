local platform = require "utils.platform"

---@type { max_fps: number?, front_end: ("OpenGL"|"Software"|"WebGpu")?, hyperlink_rules: { regex: string, format: string }[] }
return {
  max_fps = platform.is_mac and 120 or nil,
  front_end = platform.is_mac and "WebGpu" or nil,
  -- one URL rule instead of wezterm's six: a mux pane rescans every changed line on every change
  hyperlink_rules = { { regex = "\\b\\w+://\\S+[)/a-zA-Z0-9-]+", format = "$0" } },
}
