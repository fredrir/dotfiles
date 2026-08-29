local wezterm = require "wezterm"
local act = wezterm.action

local cursor_motion_keys = {
  { key = "LeftArrow", mods = M.ALT, action = act.SendKey { key = "b", mods = "ALT" } },
  { key = "RightArrow", mods = M.ALT, action = act.SendKey { key = "f", mods = "ALT" } },
  { key = "LeftArrow", mods = M.SUPER, action = act.SendKey { key = "a", mods = "CTRL" } },
  { key = "RightArrow", mods = M.SUPER, action = act.SendKey { key = "e", mods = "CTRL" } },
}

return cursor_motion_keys
