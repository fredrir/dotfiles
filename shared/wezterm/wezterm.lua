local wezterm = require "wezterm" ---@type Wezterm
local config = wezterm.config_builder() ---@type Config
config:set_strict_mode(true)

local modules = {
  "domain",
  "keymap",
  "ui",
  -- "plugins",
}

for _, module in ipairs(modules) do
  require(module).apply_to_config(config)
end

return config
