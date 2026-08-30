local wezterm = require "wezterm"

return wezterm.action_callback(function(_window, pane)
  local vars = pane:get_user_vars()

  if vars.manual_ssh ~= "1" then
    return
  end

  pane:send_text "\r~."
end)
