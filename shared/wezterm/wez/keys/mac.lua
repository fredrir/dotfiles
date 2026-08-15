local wezterm = require 'wezterm'
local act = wezterm.action
local actions = require 'wez.keys.actions'
local platform = require 'wez.platform'

local M = { keys = {} }

-- The standard macOS chords, restated because disable_default_key_bindings
-- wipes them together with everything else.
if platform.is_mac then
  M.keys = {
    { key = 'q', mods = 'CMD', action = act.QuitApplication },
    { key = 'h', mods = 'CMD', action = act.HideApplication },
    { key = 'm', mods = 'CMD', action = act.Hide },
    { key = 'n', mods = 'CMD', action = act.SpawnWindow },
    { key = 't', mods = 'CMD', action = act.SpawnTab 'CurrentPaneDomain' },
    { key = 'w', mods = 'CMD', action = act.CloseCurrentTab { confirm = false } },
    { key = 'd', mods = 'CMD', action = act.SplitHorizontal { domain = 'CurrentPaneDomain' } },
    { key = 'd', mods = 'CMD|SHIFT', action = act.SplitVertical { domain = 'CurrentPaneDomain' } },

    -- Copy the selection or do nothing: an empty CopyTo would clobber
    -- whatever tmux just put on the clipboard via OSC 52.
    { key = 'c', mods = 'CMD', action = actions.copy_or(nil) },
    { key = 'v', mods = 'CMD', action = act.PasteFrom 'Clipboard' },
    { key = 'f', mods = 'CMD', action = act.Search 'CurrentSelectionOrEmptyString' },
    { key = 'k', mods = 'CMD', action = act.ClearScrollback 'ScrollbackOnly' },
    { key = 'P', mods = 'CMD|SHIFT', action = act.ActivateCommandPalette },

    -- '+' and '-' are both unshifted on the Norwegian layout.
    { key = '+', mods = 'CMD', action = act.IncreaseFontSize },
    { key = '-', mods = 'CMD', action = act.DecreaseFontSize },
    { key = '0', mods = 'CMD', action = act.ResetFontSize },

    -- CMD+9 is "last tab", browser style; 1-8 address tabs directly.
    { key = '9', mods = 'CMD', action = act.ActivateTab(-1) },
  }
  for index = 1, 8 do
    table.insert(M.keys, {
      key = tostring(index),
      mods = 'CMD',
      action = act.ActivateTab(index - 1),
    })
  end
end

return M
