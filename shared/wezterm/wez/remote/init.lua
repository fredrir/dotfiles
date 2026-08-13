local wezterm = require 'wezterm'
local act = wezterm.action
local sessions = require 'wez.remote.sessions'
local target = require 'wez.remote.target'

local M = {}

function M.apply(config)
  local keys = config.keys or {}

  table.insert(keys, { key = 's', mods = 'LEADER', action = sessions.picker() })
  table.insert(keys, {
    key = 'a',
    mods = 'LEADER',
    action = act.SpawnCommandInNewTab { args = target.attach_command 'main' },
  })

  config.keys = keys
end

return M
