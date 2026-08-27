local wezterm = require "wezterm"
local config = wezterm.config_builder()

config:set_strict_mode(true)

local modules = { require "domain.local_mux", require "domain.tls_mux" }

for _, module in ipairs(modules) do
  module.apply_to_config(config)
end

return config
