local wezterm = require "wezterm"
local platform = require "utils.platform"

return wezterm.action_callback(function(window, pane)
  local sequence = platform.is_linux and "\x1b[5;30012~" or "\x1b[115;9u"
  window:perform_action(wezterm.action.SendString(sequence), pane)
end)
