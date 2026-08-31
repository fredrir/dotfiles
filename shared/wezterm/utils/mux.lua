local wezterm = require "wezterm"
local act = wezterm.action

local mux = {}

mux.detach_pane = wezterm.action_callback(function(_window, pane)
  pane:move_to_new_window "__detached"
end)

mux.attach_detached = act.SwitchToWorkspace {
  name = "__detached",
}

return mux
