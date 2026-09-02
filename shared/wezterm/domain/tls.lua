local host = require "domain.hosts"

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

---@type TlsDomainClient[]
local clients = {}

for _, route in ipairs(host.target.ip) do
  local client_name = ("%s-%s"):format(host.target.hostname, route.name)
  table.insert(clients, {
    name = client_name,
    remote_address = route.address .. ":" .. host.port,
    expected_cn = host.target.hostname,
    pem_cert = pem.cert,
    pem_private_key = pem.key,
    pem_root_certs = { pem.ca },
    connect_automatically = false,
    local_echo_threshold_ms = 20,
  })
end

---@type Config
local tls_config = {
  tls_servers = servers,
  tls_clients = clients,
}

return tls_config
