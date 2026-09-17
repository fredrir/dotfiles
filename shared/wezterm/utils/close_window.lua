local wezterm = require "wezterm"

return wezterm.action_callback(function(window, _)
  local tabs = window:mux_window():tabs()

  for _, tab in ipairs(tabs) do
    tab:activate()
    window:perform_action(wezterm.action.CloseCurrentTab { confirm = false }, tab:active_pane())
  end
end)
