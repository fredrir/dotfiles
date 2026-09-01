local append_conf = require "utils.append_conf"

---@type Config
local domain_config = {}

return append_conf(domain_config, "domain.unix", "domain.tls")
