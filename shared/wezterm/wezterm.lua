local wezterm = require "wezterm" ---@type Wezterm
local config = wezterm.config_builder() ---@type Config

local modules = {
  "domain",
  "keymap",
}

for _, module in ipairs(modules) do
  require(module).apply_to_config(config)
end

return config
