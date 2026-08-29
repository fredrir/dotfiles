local wezterm = require "wezterm"
local act = wezterm.action

---@type KeySpec[]
local macos_no_NB_keys = { -- Fixes ⌥+7, ⌥+8 --> [ , ]
  { key = "phys:8", mods = "OPT", action = act.SendString "[" },
  { key = "phys:9", mods = "OPT", action = act.SendString "]" },

  { key = "phys:8", mods = "OPT|SHIFT", action = act.SendString "{" },
  { key = "phys:9", mods = "OPT|SHIFT", action = act.SendString "}" },

  { key = "phys:7", mods = "OPT|SHIFT", action = act.SendString "\\" },
}

return macos_no_NB_keys
