local wezterm = require "wezterm"
local MOD = require "keymap.modifiers"
local act = wezterm.action

---@type KeySpec[]
local motion_keys = {
  { key = "LeftArrow", mods = MOD.ALT, action = act.SendKey { key = "b", mods = "ALT" } },
  { key = "RightArrow", mods = MOD.ALT, action = act.SendKey { key = "f", mods = "ALT" } },
  { key = "LeftArrow", mods = MOD.SUPER, action = act.SendKey { key = "a", mods = "CTRL" } },
  { key = "RightArrow", mods = MOD.SUPER, action = act.SendKey { key = "e", mods = "CTRL" } },
}

return motion_keys
