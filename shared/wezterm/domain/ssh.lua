local ssh_hosts = require "domain.ssh-hosts"

---@return Config
return function()
  ---@type SshDomain[]
  local domains = {}

  for _, name in ipairs(ssh_hosts()) do
    table.insert(domains, {
      name = name,
      remote_address = name,
      multiplexing = "None",
    })
  end

  return {
    ssh_domains = domains,
  }
end
