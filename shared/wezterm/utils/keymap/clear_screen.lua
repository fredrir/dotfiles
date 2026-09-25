local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action

local clear_screen = act.Multiple {
  act.ClearScrollback "ScrollbackAndViewport",
  act.SendKey { key = "L", mods = "CTRL" },
}

return clear_screen
