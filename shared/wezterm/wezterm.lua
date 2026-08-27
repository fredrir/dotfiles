local wezterm = require "wezterm" ---@type Wezterm
local config = wezterm.config_builder() ---@type Config

config:set_strict_mode(true)

local modules = { (require "domain.local_mux"), (require "domain.tls_mux") }

for _, module in ipairs(modules) do
  module.apply_to_config(config)
end

return config
