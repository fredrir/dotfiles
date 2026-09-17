local wezterm = require "wezterm" ---@type Wezterm

---@class SshMuxHost
---@field home string
---@field wezterm string

---@type table<string, SshMuxHost>
local hosts = {
  ntnu = { home = "/home/ubuntu", wezterm = "/home/ubuntu/.local/bin/wezterm" },
}

local supported = false
for _, kind in ipairs(wezterm.mux.local_pane_layout_domains or {}) do
  supported = supported or kind == "unix"
end

return {
  hosts = hosts,
  supported = supported,
}
