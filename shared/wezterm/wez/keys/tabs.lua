local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

M.keys = {
  -- CTRL+T belongs to fzf's file widget; new tab lives on the shifted chord
  -- here and on CMD+T on mac.
  { key = 't', mods = 'CTRL|SHIFT', action = act.SpawnTab 'CurrentPaneDomain' },
  -- Confirmed: CTRL+w is backward-kill-word in readline and is easy to hit.
  { key = 'w', mods = 'CTRL', action = act.CloseCurrentTab { confirm = true } },

  { key = 'Tab', mods = 'CTRL', action = act.ActivateTabRelative(1) },
  { key = 'Tab', mods = 'CTRL|SHIFT', action = act.ActivateTabRelative(-1) },

  -- '[' and ']' need AltGr here, so leader tab cycling uses n/p instead.
  { key = 'p', mods = 'LEADER|SHIFT', action = act.ActivateTabRelative(-1) },
  { key = 'n', mods = 'LEADER|SHIFT', action = act.ActivateTabRelative(1) },

  {
    key = ',',
    mods = 'LEADER',
    action = act.PromptInputLine {
      description = 'Rename tab',
      action = wezterm.action_callback(function(window, _pane, line)
        if line and #line > 0 then
          window:active_tab():set_title(line)
        end
      end),
    },
  },
}

for index = 1, 9 do
  table.insert(M.keys, {
    key = tostring(index),
    mods = 'CTRL',
    action = act.ActivateTab(index - 1),
  })
end

table.insert(M.keys, { key = '0', mods = 'CTRL', action = act.ActivateTab(-1) })

return M
