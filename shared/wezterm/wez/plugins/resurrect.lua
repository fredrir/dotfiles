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
  local wez_first = os.getenv 'DMUX_WEZ_FIRST' == '1'

  local opts = {
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
  }

  -- dmux wez-first flag (plan §15.2, P5): when the GUI is an attach-only
  -- client of the service-owned mux, gui-startup must not restore state into
  -- the already-populated server; the mux service owns cold recovery. Flag
  -- off leaves opts exactly as above, so setup() behaves identically to the
  -- pre-dmux configuration. (The fork's dmux-split branch implements
  -- startup_restore; a plugin checkout without it ignores the extra key.)
  if wez_first then
    opts.startup_restore = false
  end

  resurrect.setup(config, opts)

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
  -- A manual resurrection picker restores by spawning native Wez resources,
  -- so it is a legacy-only UI.  In Wez-first mode cold recovery is the sole
  -- restore path and is guarded by the owner coordinator; logical picker
  -- behavior is provided by dmux's non-creating workspace picker instead.
  if not wez_first then
    table.insert(keys, {
      key = 'q',
      mods = 'LEADER',
      action = resurrect.fuzzy_loader.restore_action(),
    })
  end
  table.insert(keys, {
    key = 'S',
    mods = 'LEADER|SHIFT',
    action = resurrect.workspace_state.save_workspace_action(),
  })
  config.keys = keys
end

return M
