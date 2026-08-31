local wezterm = require("wezterm")

local M = {}

function M.apply_to_config(config)
	config.unix_domains = {
		{
			name = "localmux",
			socket_path = wezterm.home_dir .. "/.local/share/wezterm/localmux.sock",
		},
	}
	config.default_domain = "localmux"
	config.default_gui_startup_args = { "connect", "localmux" }
end

return M
