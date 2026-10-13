local wezterm = require "wezterm" ---@type Wezterm

return wezterm.action_callback(function(window, pane)
  if pane:get_user_vars().WEZTERM_PROG == "" then
    window:perform_action(wezterm.action.SendString "\x1b[5;30021~", pane)
  end
end)