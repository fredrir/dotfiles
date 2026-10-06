local wezterm = require "wezterm"

return wezterm.action_callback(function(window, pane)
  local host = os.getenv "HOST"

  if host == "macie" then
    pane:send_text "attach_mux archie\n"
  elseif host == "archie" then
    pane:send_text "attach_mux macie\n"
  end
end)
