local host = require "domain.hosts"

local M = {}

function M.apply_to_config(config)
  local pem = host.origin.pem

  ---@type TlsDomainServer[]
  local servers = {}

  for _, route in ipairs(host.origin.ip) do
    table.insert(servers, {
      bind_address = route.bind,
      pem_cert = pem.cert,
      pem_private_key = pem.key,
      pem_root_certs = { pem.ca },
    })
  end

  config.tls_servers = servers

  ---@type TlsDomainClient[]
  local clients = {}

  for _, route in ipairs(host.target.ip) do
    ---@diagnostic disable-next-line: missing-fields
    table.insert(clients, {
      name = host.target.hostname .. "-" .. route.name,
      remote_address = route.address .. ":" .. host.port,
      expected_cn = host.target.hostname,
      pem_cert = pem.cert,
      pem_private_key = pem.key,
      pem_root_certs = { pem.ca },
      connect_automatically = false,
    })
  end

  config.tls_clients = clients
end

return M
