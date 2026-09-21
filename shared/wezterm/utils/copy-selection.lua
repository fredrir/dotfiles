local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action

local zle_copy = act.SendString "\x1b[99;9u"

return wezterm.action_callback(function(window, pane)
  if window:get_selection_text_for_pane(pane) ~= "" then
    window:perform_action(act.CopyTo "Clipboard", pane)
  elseif pane:get_user_vars().ZLE_SELECTION == "1" then
    window:perform_action(zle_copy, pane)
  end
end)
