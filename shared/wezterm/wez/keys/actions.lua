local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

-- Copy the selection and then drop it (so a repeat press falls through to
-- the fallback), or run the fallback action when nothing is selected. Keeps
-- CTRL+C usable as both "copy" and "interrupt", and stops CMD+C from
-- clobbering the clipboard with an empty string.
function M.copy_or(fallback)
  return wezterm.action_callback(function(window, pane)
    local selection = window:get_selection_text_for_pane(pane)
    if selection and #selection > 0 then
      window:perform_action(act.Multiple { act.CopyTo 'Clipboard', act.ClearSelection }, pane)
    elseif fallback then
      window:perform_action(fallback, pane)
    end
  end)
end

return M
