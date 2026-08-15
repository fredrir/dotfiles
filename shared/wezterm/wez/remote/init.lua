local wezterm = require 'wezterm'
local act = wezterm.action
local mux = require 'wez.remote.mux'
local sessions = require 'wez.remote.sessions'

local M = {}

function M.apply(config)
  -- Replaces wezterm's auto-generated ssh domain list with exactly ours.
  config.ssh_domains = mux.domains()

  local keys = config.keys or {}
  table.insert(keys, { key = 'a', mods = 'LEADER', action = mux.attach_action() })
  table.insert(keys, { key = 'd', mods = 'LEADER', action = act.DetachDomain 'CurrentPaneDomain' })
  table.insert(keys, { key = 's', mods = 'LEADER', action = sessions.picker() })
  config.keys = keys
end

return M
