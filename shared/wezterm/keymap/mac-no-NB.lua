local wezterm = require "wezterm"
local act = wezterm.action

---@class Key
---@field key string
---@field mods string
---@field

local macos_no_NB_keys = {
  { key = "phys:8", mods = "OPT", action = act.SendString "[" },
  { key = "phys:9", mods = "OPT", action = act.SendString "]" },

  { key = "phys:8", mods = "OPT|SHIFT", action = act.SendString "{" },
  { key = "phys:9", mods = "OPT|SHIFT", action = act.SendString "}" },

  { key = "phys:7", mods = "OPT|SHIFT", action = act.SendString "\\" },
}

table.move(macos_no_NB_keys, 1, #macos_no_NB_keys, #keys + 1, keys)
