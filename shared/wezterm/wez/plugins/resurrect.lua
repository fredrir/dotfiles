local wezterm = require 'wezterm'

-- Workspace persistence: periodic + focus-loss + structure-change saves, a
-- fuzzy restore picker, and restore-on-startup of the last active workspace.
-- State is plain JSON under the per-OS state dir; treat it as
-- config-equivalent (it holds scrollback text) and never sync it between
-- machines.
local M = {}

local URL = 'https://github.com/fredrir/resurrect.wezterm'

function M.apply(config)
  local resurrect = wezterm.plugin.require(URL)

  resurrect.setup(config, {
    periodic_interval = 300,
    keybindings = false,
    -- wez.appearance.status owns update-right-status.
    status_bar = false,
    -- Workspace-level state only.
    save_windows = false,
    save_tabs = false,
    -- Never re-type saved commands into a shell on restore: the allowlist
    -- gate checks process.name but executes process.argv, so a tampered
    -- state file could otherwise run arbitrary commands.
    safe_restore_processes = { replace = {} },
  })

  -- Keep state local-only: strip remote-domain markers (the archie/macie
  -- mux domains) at save time. Their panes restore as local shells in the
  -- same layout slot; without this a restore auto-dials ssh at gui-startup,
  -- and an unreachable peer aborts the rest of that window's tabs.
  local get_workspace_state = resurrect.workspace_state.get_workspace_state
  resurrect.workspace_state.get_workspace_state = function()
    local state = get_workspace_state()
    for _, window in ipairs(state.window_states or {}) do
      for _, tab in ipairs(window.tabs or {}) do
        if tab.pane_tree then
          resurrect.pane_tree.map(tab.pane_tree, function(node)
            local domain = node.domain
            if domain and domain ~= 'local' and not domain:find '^unix' then
              node.domain = nil
              node.cwd = ''
            end
            return node
          end)
        end
      end
    end
    return state
  end

  local keys = config.keys or {}
  table.insert(keys, {
    key = 'q',
    mods = 'LEADER',
    action = resurrect.fuzzy_loader.restore_action(),
  })
  table.insert(keys, {
    key = 'S',
    mods = 'LEADER|SHIFT',
    action = resurrect.workspace_state.save_workspace_action(),
  })
  config.keys = keys
end

return M
