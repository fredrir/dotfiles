---@class BindKey
---@field key string
---@field mods string|string[]
---@field action Action

---@param bindings BindKey[]
---@return Key[]
local function bind_keys(bindings)
	---@type Key[]
	local keys = {}

	for _, binding in ipairs(bindings) do
		local mods = binding.mods

		if type(mods) == "string" then
			mods = { mods }
		end

		for _, mod in ipairs(mods) do
			table.insert(keys, {
				key = binding.key,
				mods = mod,
				action = binding.action,
			})
		end
	end

	return keys
end

return bind_keys
