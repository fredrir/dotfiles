local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action
local runtime = require "utils.runtime"

local close_tab = wezterm.action_callback(function(window, pane)
  local confirm = runtime.tab_has_running_process(window:active_tab())
  window:perform_action(act.CloseCurrentTab { confirm = confirm }, pane)
end)

return close_tab
