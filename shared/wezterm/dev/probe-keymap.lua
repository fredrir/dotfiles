---@type table<string, string>
local TRIPLES = {
	mac = "aarch64-apple-darwin",
	linux = "x86_64-unknown-linux-gnu",
	windows = "x86_64-pc-windows-msvc",
}

---@type table<string, string>
local RESERVED_CONTROL_CODES = {
	c = "SIGINT",
	v = "literal-next",
	n = "next-history",
	t = "transpose-chars",
	l = "clear-screen",
}

---@param name string
---@return table
local function action_marker(name)
	return setmetatable({ __action = name }, {
		__call = function(_, a)
			return { __action = name, arg = a }
		end,
	})
end

---@param triple string
---@return table
local function stub_wezterm(triple)
	return {
		target_triple = triple,
		home_dir = os.getenv("HOME") or "/home/user",
		hostname = function()
			return "probe"
		end,
		action = setmetatable({}, {
			__index = function(_, k)
				return action_marker(k)
			end,
		}),
		action_callback = function()
			return { __action = "callback" }
		end,
		font_with_fallback = function(t)
			return { font = t }
		end,
		log_error = function() end,
		log_info = function() end,
		gui = nil,
	}
end

---@param action any
---@return string
local function render(action)
	if type(action) ~= "table" then
		return tostring(action)
	end
	local out = tostring(action.__action or "?")
	if action.arg == nil then
		return out
	end
	if type(action.arg) ~= "table" then
		return out .. "(" .. tostring(action.arg) .. ")"
	end
	---@type table<string, any>
	local args = action.arg
	---@type string[]
	local parts = {}
	for k, v in pairs(args) do
		parts[#parts + 1] = tostring(k) .. "=" .. (type(v) == "table" and "{...}" or tostring(v))
	end
	table.sort(parts)
	return out .. "{" .. table.concat(parts, ",") .. "}"
end

---@param triple string
---@return table[]
local function collect(triple)
	---@type table<string, any>
	local loaded = package.loaded
	for name in pairs(loaded) do
		if name:match("^keymap") or name:match("^utils") or name:match("^ui") then
			loaded[name] = nil
		end
	end
	package.preload["wezterm"] = function()
		return stub_wezterm(triple)
	end
	loaded["wezterm"] = nil

	---@type table
	local config = {}
	require("keymap").apply_to_config(config)
	---@type table[]
	local keys = config.keys
	return keys
end

---@param keys table[]
---@return string[]
local function shadowed(keys)
	---@type string[]
	local hits = {}
	for _, binding in ipairs(keys) do
		local key = tostring(binding.key)
		local meaning = RESERVED_CONTROL_CODES[key:lower()]
		local passthrough = ("SendKey{key=%s,mods=CTRL}"):format(key)
		if meaning and binding.mods == "CTRL" and render(binding.action) ~= passthrough then
			hits[#hits + 1] = ("Ctrl+%s shadows %s with %s"):format(key:upper(), meaning, render(binding.action))
		end
	end
	return hits
end

local root = tostring(arg[0]):gsub("dev/probe%-keymap%.lua$", "")
package.path = root .. "?.lua;" .. root .. "?/init.lua;" .. package.path

---@type table<string, string>
local selected = arg[1] and { [arg[1]] = TRIPLES[arg[1]] or arg[1] } or TRIPLES

---@type string[]
local names = {}
for name in pairs(selected) do
	names[#names + 1] = name
end
table.sort(names)

local failed = false
for _, name in ipairs(names) do
	local keys = collect(selected[name])
	print(("--- %s (%s): %d bindings ---"):format(name, selected[name], #keys))

	---@type string[]
	local rows = {}
	for _, binding in ipairs(keys) do
		rows[#rows + 1] = ("  %-14s %-14s %s"):format(
			tostring(binding.mods),
			tostring(binding.key),
			render(binding.action)
		)
	end
	table.sort(rows)
	print(table.concat(rows, "\n"))

	local hits = shadowed(keys)
	if #hits > 0 then
		failed = true
		print("  FAIL: bare-Ctrl bindings shadow reserved control codes")
		for _, hit in ipairs(hits) do
			print("     " .. hit)
		end
	else
		print("  ok: no bare-Ctrl binding shadows a reserved control code")
	end
	print()
end

os.exit(failed and 1 or 0)
