local host = require "domain.hosts"

local M = {}

function M.apply_to_config(config)
  local port = "8080"

  config.tls_servers = { {
    bind_address = host.origin.ip[1].address .. ":" .. port,
  } }

  config.tls_clients = {
    {
      name = host.target.hostname .. "-tls",
      remote_address = host.target.ip[1].address .. ":" .. port,
      bootstrap_via_ssh = host.target.hostname,
      expected_cn = host.target.hostname,
      remote_wezterm_path = host.target.wezterm,
    },
  }
end

return M
