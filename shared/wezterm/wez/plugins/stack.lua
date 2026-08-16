local wezterm = require 'wezterm'

-- Kitty-style stack workflow on top of wezterm's native zoom: spawn a pane
-- that stays stacked, and cycle panes without leaving the maximized view.
-- LEADER+z remains the stack <-> split toggle.
local M = {}

local URL = 'https://github.com/fredrir/stack.wez'

function M.apply(config)
  if os.getenv 'DMUX_WEZ_FIRST' == '1' then
    -- SpawnPane bypasses dmux identity/owner validation. The managed-mode
    -- replacement is the logical Split actions installed by the bridge.
    return
  end
  local stack = wezterm.plugin.require(URL)

  -- Deliberately not calling stack.apply_to_config: it would only register
  -- a format-tab-title handler that fights wez.appearance.tabs.
  local keys = config.keys or {}
  table.insert(keys, { key = 't', mods = 'LEADER', action = stack.action.SpawnPane })
  table.insert(keys, { key = 'o', mods = 'LEADER', action = stack.action.ActivatePaneRelative(1) })
  table.insert(keys, { key = 'i', mods = 'LEADER', action = stack.action.ActivatePaneRelative(-1) })
  config.keys = keys
end

return M
