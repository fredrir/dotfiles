local wezterm = require 'wezterm'
local config = wezterm.config_builder()

-- W6 managed mode is a fail-closed security boundary, not an optional
-- feature module.  Install its empty/default-deny surfaces before any
-- legacy module can append keys or launch actions; a missing bridge or key
-- must abort this config instead of falling back to stock spawn/quit paths.
local dmux_bridge
if os.getenv 'DMUX_WEZ_FIRST' == '1' then
  dmux_bridge = require 'wez.dmux_bridge'
  dmux_bridge.preflight(config)
end

-- Order matters: wez.keys establishes config.keys, and the modules after it
-- append their own bindings to that table.
local modules = {
  'wez.appearance',
  'wez.perf',
  'wez.keys',
  'wez.domains',
  'wez.remote',
  'wez.integrations',
  -- Last: wez.plugins.sync mirrors the paste bindings it finds in
  -- config.keys, so every key module must have run already.
  'wez.plugins',
}

for _, name in ipairs(modules) do
  -- A broken module degrades to "that feature is missing" rather than taking
  -- the whole config down to stock defaults.
  local ok, err = pcall(function()
    local mod = require(name)
    if mod.apply then
      mod.apply(config)
    end
    if mod.setup then
      mod.setup()
    end
  end)
  if not ok then
    -- The managed local domain is the load-bearing no-create boundary. A
    -- missing/starting/failed descriptor must abort flag-on config loading;
    -- swallowing it would let WezTerm fall back to an unmanaged default
    -- shell on direct app startup.
    if dmux_bridge and name == 'wez.domains' then
      error('dmux managed domain failed: ' .. tostring(err))
    end
    wezterm.log_error('wezterm config: module ' .. name .. ' failed: ' .. tostring(err))
  end
end

-- Sanitize after every legacy module (including plugins) has had its turn,
-- then register the authenticated consumer and managed-close handler.  These
-- calls deliberately are not protected by pcall in flag-on mode.
if dmux_bridge then
  dmux_bridge.apply(config)
  dmux_bridge.setup()
end

return config
