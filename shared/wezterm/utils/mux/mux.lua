local wezterm = require "wezterm"

local mux = {}

mux.localmux_socket = wezterm.home_dir .. "/.local/share/wezterm/localmux.sock"

mux.detach_pane = wezterm.action_callback(function(_window, pane)
  pane:move_to_new_window "__detached"
end)

return mux