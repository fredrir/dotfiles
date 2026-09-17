local append_conf = require "utils.append_conf"
require "utils.attach-mux"

---@return Config
return function()
  return append_conf({}, "domain.unix", "domain.tls", "domain.ssh")
end
