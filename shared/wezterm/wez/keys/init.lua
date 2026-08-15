local wezterm = require 'wezterm'
local leader = require 'wez.keys.leader'

local M = {}

local SOURCES = {
  'wez.keys.panes',
  'wez.keys.tabs',
  'wez.keys.copy',
  'wez.keys.window',
  'wez.keys.mac',
}

function M.apply(config)
  -- The leader is set first and unconditionally: later modules append LEADER
  -- bindings, and a leader-less config would leave those permanently dead.
  config.leader = leader.leader

  -- Defaults stay off. On a non-US layout a shifted binding such as CTRL+SHIFT+8
  -- produces a different character, which would otherwise land on an unrelated
  -- default action instead of doing nothing visible.
  config.disable_default_key_bindings = true

  local keys = {}
  local key_tables = {}

  local function collect(source)
    for _, key in ipairs(source.keys or {}) do
      table.insert(keys, key)
    end
    for name, entries in pairs(source.key_tables or {}) do
      key_tables[name] = entries
    end
  end

  collect(leader)
  for _, name in ipairs(SOURCES) do
    -- Per module, so one broken keymap costs only its own bindings.
    local ok, err = pcall(function()
      collect(require(name))
    end)
    if not ok then
      wezterm.log_error('wezterm keys: ' .. name .. ' failed: ' .. tostring(err))
    end
  end

  config.keys = keys
  config.key_tables = key_tables
end

return M
