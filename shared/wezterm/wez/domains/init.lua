local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

local DOMAIN = 'unix'

function M.apply(config)
  config.unix_domains = { {
    name = DOMAIN,
  } }

  local keys = config.keys or {}
  table.insert(keys, {
    key = 'u',
    mods = 'LEADER',
    action = act.SpawnCommandInNewTab {
      domain = {
        DomainName = DOMAIN,
      },
    },
  })
  -- Detaching is LEADER+d (wez.remote), which works for any domain.
  table.insert(keys, {
    key = 'A',
    mods = 'LEADER',
    action = act.AttachDomain(DOMAIN),
  })
  config.keys = keys
end

return M
