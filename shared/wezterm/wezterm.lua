local wezterm = require 'wezterm'
local config = wezterm.config_builder()

-- Order matters: wez.keys establishes config.keys, and the modules after it
-- append their own bindings to that table.
local modules = {
  'wez.appearance',
  'wez.keys',
  'wez.domains',
  'wez.remote',
  'wez.integrations',
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
    wezterm.log_error('wezterm config: module ' .. name .. ' failed: ' .. tostring(err))
  end
end

return config
