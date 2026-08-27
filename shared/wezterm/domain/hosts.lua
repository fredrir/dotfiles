local wezterm = require "wezterm"

---@class Host
---@field hostname string
---@field target string
---@field ip {name: string, address: string}[]
---@field wezterm string

---@type table<string, Host>
local hosts = {
  macie = {
    hostname = "macie",
    target = "archie",
    ip = {
      {
        name = "cable",
        address = "10.77.77.1",
      },
      {
        name = "wifi",
        address = "10.77.78.1",
      },
      {
        name = "tailscale",
        address = "100.75.71.79",
      },
    },
    wezterm = "/opt/homebrew/bin/wezterm",
  },
  archie = {
    hostname = "archie",
    target = "macie",
    ip = {
      {
        name = "cable",
        address = "10.77.77.2",
      },
      {
        name = "wifi",
        address = "10.77.78.2",
      },
      {
        name = "tailscale",
        address = "100.126.231.24",
      },
    },
    wezterm = "/usr/bin/wezterm",
  },
}

local origin = hosts[wezterm.hostname()]
assert(origin, "Unknown host: " .. wezterm.hostname())

local target = hosts[origin.target]

return {
  origin = origin,
  target = target,
}
