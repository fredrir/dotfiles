local wezterm = require 'wezterm'
local platform = require 'wez.platform'

-- Zoxide-driven workspace switcher: existing workspaces first, then zoxide
-- directories; picking a directory creates a workspace rooted there.
local M = {}

local URL = 'https://github.com/fredrir/workspace-picker.wezterm'

function M.apply(config)
  local picker = wezterm.plugin.require(URL)

  picker.setup {
    zoxide_path = platform.pick {
      mac = '/opt/homebrew/bin/zoxide',
      default = '/usr/bin/zoxide',
    },
    -- false, not nil: a nil field is indistinguishable from absent, so the
    -- plugin would re-install its default LEADER+s binding over the tmux
    -- session picker.
    keybinds = false,
  }

  local keys = config.keys or {}
  table.insert(keys, {
    key = 'w',
    mods = 'LEADER',
    action = wezterm.action_callback(function(window, pane)
      picker.show_workspace_selector(window, pane)
    end),
  })
  config.keys = keys
end

return M
