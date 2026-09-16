local wezterm = require "wezterm"

---@type Config
local ssh_domains = {
  ssh_domains = {
    {
      name = "ntnu",
      remote_address = "10.99.50.100",
      username = "ubuntu",
    },
  },
}

return ssh_domains
