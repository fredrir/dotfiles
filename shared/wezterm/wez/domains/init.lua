local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

local DOMAIN = 'unix'

function M.apply(config)
  -- Panes spawned into this domain outlive the GUI process. It is deliberately
  -- not connected automatically: a mux server failure must never stop the
  -- terminal from starting.
  config.unix_domains = { { name = DOMAIN } }

  local keys = config.keys or {}
  table.insert(keys, {
    key = 'u',
    mods = 'LEADER',
    action = act.SpawnCommandInNewTab { domain = { DomainName = DOMAIN } },
  })
  table.insert(keys, {
    key = 'U',
    mods = 'LEADER',
    action = act.DetachDomain { DomainName = DOMAIN },
  })
  -- Without this the panes that outlive the GUI are unreachable after a detach
  -- or a restart.
  table.insert(keys, {
    key = 'A',
    mods = 'LEADER',
    action = act.AttachDomain(DOMAIN),
  })
  config.keys = keys
end

return M
