local wezterm = require 'wezterm'

-- Marker-file driven tab indicators (agent thinking/done/needs-input, plus
-- the bell bridge in wez.appearance.tabs). Rendering happens in the custom
-- format-tab-title handler; here the plugin only gets its cache poller and
-- the review-toggle key. Runs after wez.keys so the binding survives.
local M = {}

local URL = 'https://github.com/fredrir/wezterm-attention'

function M.apply(config)
  local attention = wezterm.plugin.require(URL)

  attention.apply_to_config(config, {
    -- Keep the custom format-tab-title handler; the plugin registers none.
    -- Its update-status handler only fills the marker cache and composes
    -- with wez.appearance.status.
    renderer = 'manual',
    -- The ALT+b default needs AltGr on the Norwegian layout and composes
    -- on mac; LEADER+m marks the current pane for review instead.
    review_key = { key = 'm', mods = 'LEADER' },
  })
end

return M
