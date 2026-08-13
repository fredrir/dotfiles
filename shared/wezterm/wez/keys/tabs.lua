local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

M.keys = {
  { key = 't', mods = 'CTRL', action = act.SpawnTab 'CurrentPaneDomain' },
  -- Confirmed: CTRL+w is backward-kill-word in readline and is easy to hit.
  { key = 'w', mods = 'CTRL', action = act.CloseCurrentTab { confirm = true } },

  -- '[' and ']' need AltGr here, so tab cycling uses n/p instead.
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
