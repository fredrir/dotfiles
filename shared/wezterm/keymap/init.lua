local wezterm = require "wezterm" ---@type Wezterm

local M = {}

-- The chord types the `mux` shell function rather than reaching for the peer
-- itself. The gui's lua runs against its own mux, whose pane ids are not the
-- ones `wezterm cli` and $WEZTERM_PANE speak, so a tab spawned from here
-- lands outside localmux and a close cannot be aimed at this pane -- it takes
-- whichever pane is active instead. The shell function holds the ids that
-- work, and its errors land in the shell that asked rather than in a toast.
M.attach = wezterm.action_callback(function(_, pane)
  pane:send_text "mux\n"
end)

function M.apply_to_config(config)
  local platform = wezterm.target_triple:find "darwin" and "keymap.macos" or "keymap.linux"
  local chord = require(platform)

  config.keys = {
    { key = chord.key, mods = chord.mods, action = M.attach },
  }
end

return M
