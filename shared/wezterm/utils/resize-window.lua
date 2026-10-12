local wezterm = require "wezterm"
local act = wezterm.action

return function(grow)
  return wezterm.action_callback(function(window, pane)
    local tab = pane:tab()
    local function direction(forward, back)
      local edge = tab ~= nil and tab:get_pane_direction(forward) == nil and tab:get_pane_direction(back) ~= nil
      return grow ~= edge and forward or back
    end
    window:perform_action(act.AdjustPaneSize { direction("Right", "Left"), 3 }, pane)
    window:perform_action(act.AdjustPaneSize { direction("Down", "Up"), 1 }, pane)
  end)
end