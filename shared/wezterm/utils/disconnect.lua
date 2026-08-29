local wezterm = require "wezterm"

local act = wezterm.action

return wezterm.action_callback(function(window, pane)
  -- detatch if mux
  local domain = pane:get_domain_name()

  if domain ~= "local" then
    window:perform_action(act.DetachDomain "CurrentPaneDomain", pane)
    return
  end

  -- exit if normal ssh
  local process = pane:get_foreground_process_name()
  if not process then
    return
  end

  local name = process:match "([^/\\]+)$"

  if name == "ssh" or name == "ssh.exe" then
    pane:send_text "\r~."
  end
end)
