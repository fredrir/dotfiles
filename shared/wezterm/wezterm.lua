local wezterm = require "wezterm"
local config = wezterm.config_builder()

local modules = {
  "domain",
  "keymap",
  "ui",
  "plugins",
}

for _, module in ipairs(modules) do
  require(module).apply_to_config(config)
end

return config
