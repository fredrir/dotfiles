local wezterm = require "wezterm"

---@diagnostic disable: missing-fields
---@type Config
local unix_config = {
  unix_domains = {
    {
      name = "localmux",
      socket_path = wezterm.home_dir .. "/.local/share/wezterm/localmux.sock",
      no_serve_automatically = true,
    },
  },
  default_domain = "localmux",
  default_gui_startup_args = { "connect", "localmux" },
}
---@diagnostic enable: missing-fields

return unix_config
