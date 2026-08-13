local wezterm = require 'wezterm'
local platform = require 'wez.platform'
local act = wezterm.action

-- CTRL+b is free because tmux moved to CTRL+Space; this keeps CTRL+a as
-- readline beginning-of-line. Meta is deliberately unused: AltGr owns it on
-- the Norwegian layout and | [ ] \ { } are unreachable without it.
local M = {
  leader = { key = 'b', mods = 'CTRL', timeout_milliseconds = 1000 },
  keys = {
    { key = 'b', mods = 'LEADER|CTRL', action = act.SendKey { key = 'b', mods = 'CTRL' } },
  },
}

if platform.is_mac then
  local cmd = {
    { key = 't', mods = 'CMD', action = act.SpawnTab 'CurrentPaneDomain' },
    { key = 'w', mods = 'CMD', action = act.CloseCurrentTab { confirm = false } },
    { key = 'n', mods = 'CMD', action = act.SpawnWindow },
    { key = 'd', mods = 'CMD', action = act.SplitHorizontal { domain = 'CurrentPaneDomain' } },
    { key = 'd', mods = 'CMD|SHIFT', action = act.SplitVertical { domain = 'CurrentPaneDomain' } },
    { key = 'c', mods = 'CMD', action = act.CopyTo 'Clipboard' },
    { key = 'v', mods = 'CMD', action = act.PasteFrom 'Clipboard' },
    { key = 'f', mods = 'CMD', action = act.Search 'CurrentSelectionOrEmptyString' },
    { key = 'k', mods = 'CMD', action = act.ActivateCommandPalette },
  }
  for _, key in ipairs(cmd) do
    table.insert(M.keys, key)
  end
end

return M
