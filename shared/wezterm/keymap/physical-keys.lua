local wezterm = require("wezterm")
-- local extend = require "utils.extend"
local act = wezterm.action

---@type KeySpec[]
local physical_keys = {
	-- Fixes ⌥+7 '[', ⌥+8 ']',
	{ key = "phys:8", mods = "OPT", action = act.SendString("[") },
	{ key = "phys:9", mods = "OPT", action = act.SendString("]") },

	{ key = "phys:8", mods = "OPT|SHIFT", action = act.SendString("{") },
	{ key = "phys:9", mods = "OPT|SHIFT", action = act.SendString("}") },

	{ key = "phys:7", mods = "OPT|SHIFT", action = act.SendString("\\") },
}

-- extend(physical_keys{})

return physical_keys
