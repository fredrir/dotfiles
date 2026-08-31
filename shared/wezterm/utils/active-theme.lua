local colors = require("ui.colors")

local ansi = colors.get_active().ansi
assert(#ansi == 8, "active color scheme ANSI palette must contain 8 colors")

---@class ActiveTheme
---@field [1] string
---@field [2] string
---@field [3] string
---@field [4] string
---@field [5] string
---@field [6] string
---@field [7] string
---@field [8] string

---@type ActiveTheme
local active_theme = {
	ansi[1],
	ansi[2],
	ansi[3],
	ansi[4],
	ansi[5],
	ansi[6],
	ansi[7],
	ansi[8],
}

return active_theme
