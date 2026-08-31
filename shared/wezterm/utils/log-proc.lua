local wezterm = require("wezterm")

---@class ProcessNode
---@field pid integer
---@field name string
---@field status string
---@field children ProcessNode[]

---@param proc ProcessNode
---@param indent string?
---@return nil
local function log_proc(proc, indent)
	indent = indent or ""

	wezterm.log_info(indent .. "pid=" .. proc.pid .. ", name=" .. proc.name .. ", status=" .. proc.status)

	for _, child in pairs(proc.children) do
		log_proc(child, indent .. "  ")
	end
end
return log_proc
