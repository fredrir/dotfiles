local wezterm = require "wezterm"

--@alias Hostname "macie" | "archie"
--@alias InterfaceName "cable" | "wifi" | "tailscale"

--@alias Pem {
--  pem_private_key: string,
--  pem_cert: string,
--  pem_ca: string,
--}

--@alias IpAddress {
--  name: InterfaceName,
--  address: string,
--}

--@alias Host {
--  hostname: Hostname,
--  target: Hostname,
--  pem: Pem,
--  ip: IpAddress[],
--}

local pki_dir = wezterm.home_dir .. "/.local/share/wezterm/pki/"

--@type Pem
local pem = {
  pem_private_key = pki_dir .. "private_key.pem",
  pem_cert = pki_dir .. "cert.pem",
  pem_ca = pki_dir .. "ca.pem",
}

--@type table<string, Host>
local hosts = {
  macie = {
    hostname = "macie",
    target = "archie",
    pem = pem,
    ip = {
      { name = "cable", address = "10.77.77.1" },
      { name = "wifi", address = "10.77.78.1" },
      { name = "tailscale", address = "100.75.71.79" },
    },
  },

  archie = {
    hostname = "archie",
    target = "macie",
    pem = pem,
    ip = {
      { name = "cable", address = "10.77.77.2" },
      { name = "wifi", address = "10.77.78.2" },
      { name = "tailscale", address = "100.126.231.24" },
    },
  },
}

local hostname = wezterm.hostname()

local origin = assert(hosts[hostname], ("Unknown host: %s"):format(hostname))

local target = assert(hosts[origin.target], ("Unknown target host: %s"):format(origin.target))

return {
  origin = origin,
  target = target,
}
