local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action
local runtime = require "utils.runtime"

local close_pane = wezterm.action_callback(function(window, pane)
  local confirm = runtime.pane_has_running_process(pane)
  window:perform_action(act.CloseCurrentPane { confirm = confirm }, pane)
end)

return close_pane
