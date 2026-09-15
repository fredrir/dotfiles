local wezterm = require "wezterm"

---@alias Hostname "macie" | "archie"
---@alias InterfaceName "cable" | "wifi" | "lan" | "tailscale"

local pki_dir = wezterm.home_dir .. "/.local/share/wezterm/mtls/"

---@type Pem
local pem = {
  key = pki_dir .. "private_key.pem",
  cert = pki_dir .. "cert.pem",
  ca = pki_dir .. "ca.pem",
}

local port = 8443

---@type table<string, Host>
local hosts = {
  macie = {
    hostname = "macie",
    target = "archie",
    home = "/Users/fredrir",
    pem = pem,
    ip = {
      { name = "cable", address = "10.77.77.1", bind = "127.0.0.1:8443" },
      { name = "wifi", address = "10.77.78.1", bind = "127.0.0.1:8444" },
      { name = "lan", bind = "127.0.0.1:8446", dial = "127.0.0.1:8447" },
      { name = "tailscale", address = "100.75.71.79", bind = "127.0.0.1:8445" },
    },
  },

  archie = {
    hostname = "archie",
    target = "macie",
    home = "/home/fredrir",
    pem = pem,
    ip = {
      { name = "cable", address = "10.77.77.2", bind = "10.77.77.2:8443" },
      { name = "wifi", address = "10.77.78.2", bind = "10.77.78.2:8443" },
      { name = "lan", bind = "127.0.0.1:8446", dial = "127.0.0.1:8447" },
      { name = "tailscale", address = "100.124.205.100", bind = "100.124.205.100:8443" },
    },
  },
}

local hostname = wezterm.hostname()

local origin = assert(hosts[hostname], ("Unknown host: %s"):format(hostname))

local target = assert(hosts[origin.target], ("Unknown target host: %s"):format(origin.target))

---@type Hosts
local selected_hosts = {
  origin = origin,
  target = target,
  port = port,
}

return selected_hosts
