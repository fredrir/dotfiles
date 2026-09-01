local wezterm = require "wezterm"

-- wezterm-types 4.3.0 marks fields with runtime defaults as required.
---@diagnostic disable: missing-fields
---@type Config
local unix_config = {
  unix_domains = {
    {
      name = "localmux",
      socket_path = wezterm.home_dir .. "/.local/share/wezterm/localmux.sock",
    },
  },
  default_domain = "localmux",
  default_gui_startup_args = { "connect", "localmux" },
}
---@diagnostic enable: missing-fields

return unix_config
