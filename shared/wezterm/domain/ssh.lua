local wezterm = require "wezterm"

---@type Config
local ssh_domains = {
  ssh_domains = {
    {
      name = "ntnu",
      remote_address = "ntnu",
      username = "ubuntu",
      multiplexing = "WezTerm",
    },
  },
}

return ssh_domains
