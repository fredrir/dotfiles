local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

M.keys = {
  -- '+' and '-' are both unshifted on the Norwegian layout.
  { key = '+', mods = 'CTRL', action = act.IncreaseFontSize },
  { key = '-', mods = 'CTRL', action = act.DecreaseFontSize },
  { key = 'Backspace', mods = 'CTRL', action = act.ResetFontSize },

  { key = 'n', mods = 'LEADER', action = act.SpawnWindow },
  { key = 'f', mods = 'LEADER', action = act.ToggleFullScreen },
  { key = 'p', mods = 'LEADER', action = act.ActivateCommandPalette },
}

if os.getenv 'DMUX_WEZ_FIRST' == '1' then
  M.keys = {
    { key = '+', mods = 'CTRL', action = act.IncreaseFontSize },
    { key = '-', mods = 'CTRL', action = act.DecreaseFontSize },
    { key = 'Backspace', mods = 'CTRL', action = act.ResetFontSize },
    { key = 'f', mods = 'LEADER', action = act.ToggleFullScreen },
  }
end

return M
