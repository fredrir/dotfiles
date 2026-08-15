local wezterm = require 'wezterm'
local act = wezterm.action
local actions = require 'wez.keys.actions'

local M = {}

-- The terminal convention xremap gives kitty and konsole, native here:
--   CTRL+C  copies when something is selected, interrupts otherwise
--   CTRL+X  always interrupts (the dedicated abort key)
--   CTRL+V  pastes
-- CTRL+SHIFT+C is owned by wez.integrations.clean_copy.
M.keys = {
  { key = 'c', mods = 'CTRL', action = actions.copy_or(act.SendKey { key = 'c', mods = 'CTRL' }) },
  { key = 'x', mods = 'CTRL', action = act.SendKey { key = 'c', mods = 'CTRL' } },
  { key = 'v', mods = 'CTRL', action = act.PasteFrom 'Clipboard' },
  { key = 'v', mods = 'CTRL|SHIFT', action = act.PasteFrom 'Clipboard' },
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
