local unix = require("domain.unix")
local tls = require("domain.tls")

local M = {}

function M.apply_to_config(config)
	unix.apply_to_config(config)
	tls.apply_to_config(config)
end

return M
