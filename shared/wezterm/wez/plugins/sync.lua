local wezterm = require 'wezterm'

-- tmux synchronize-panes equivalent: LEADER+y broadcasts keystrokes to every
-- pane in the tab. Must apply after every other key module so the generated
-- key table can mirror the paste bindings it finds in config.keys.
local M = {}

local URL = 'https://github.com/fredrir/sync-panes.wez'

function M.apply(config)
  local sync = wezterm.plugin.require(URL)

  sync.apply_to_config(config, {
    -- The SYNC segment lives in wez.appearance.status; the plugin's own
    -- indicator never clears and its border restore misbehaves with
    -- multiple windows.
    indicator = false,
    border = false,
    -- ASCII characters that arrive shifted on the Norwegian layout. æøå and
    -- AltGr characters are outside the plugin's mirrored range and only
    -- reach the focused pane.
    needs_shift = { '!', '"', '#', '%', '&', '/', '(', ')', '=', '?', '*', ':', ';', '>', '_' },
    -- The default CTRL+SHIFT+E toggle stays: CTRL|SHIFT chords fall through
    -- the sync table, so it is the guaranteed off-switch.
  })

  table.insert(config.keys, { key = 'y', mods = 'LEADER', action = sync.toggle })

  -- While synced, key-table lookup wins over leader detection, so CTRL+b
  -- would broadcast 0x02 and the leader could never engage. Later entries
  -- in a key table win: make CTRL+b exit sync instead.
  local table_name = 'sync_mode'
  if config.key_tables and config.key_tables[table_name] then
    table.insert(config.key_tables[table_name], { key = 'b', mods = 'CTRL', action = sync.toggle })
  end
end

return M
