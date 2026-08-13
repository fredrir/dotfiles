local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

M.keys = {
  { key = 'c', mods = 'CTRL|SHIFT', action = act.CopyTo 'Clipboard' },
  { key = 'v', mods = 'CTRL|SHIFT', action = act.PasteFrom 'Clipboard' },
  { key = 'v', mods = 'CTRL', action = act.PasteFrom 'Clipboard' },
  { key = 'Insert', mods = 'CTRL', action = act.CopyTo 'Clipboard' },
  { key = 'Insert', mods = 'SHIFT', action = act.PasteFrom 'PrimarySelection' },

  { key = 'x', mods = 'CTRL|SHIFT', action = act.ActivateCopyMode },
  { key = ' ', mods = 'LEADER', action = act.QuickSelect },

  -- '/' is Shift+7 on the Norwegian layout, so the event carries SHIFT and a
  -- bare LEADER binding would never match.
  { key = '/', mods = 'LEADER|SHIFT', action = act.Search 'CurrentSelectionOrEmptyString' },

  { key = 'PageUp', mods = 'SHIFT', action = act.ScrollByPage(-1) },
  { key = 'PageDown', mods = 'SHIFT', action = act.ScrollByPage(1) },
  { key = 'k', mods = 'CTRL|SHIFT', action = act.ClearScrollback 'ScrollbackOnly' },
}

return M
