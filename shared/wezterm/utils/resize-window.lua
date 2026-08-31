local wezterm = require("wezterm") ---@type Wezterm

---@param delta number
---@return Action
local function resize_window(delta)
	return wezterm.action_callback(function(window, _pane)
		local dimensions = window:get_dimensions()

		window:set_inner_size(dimensions.pixel_width + delta, dimensions.pixel_height + delta)
	end)
end

return resize_window
