local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action

---@param pane Pane
---@return boolean
local function has_running_process(pane)
  if pane:get_user_vars().WEZTERM_PROG ~= "" then
    return true
  end

  return false
end

local close_pane = wezterm.action_callback(function(window, pane)
  window:perform_action(act.CloseCurrentPane { confirm = has_running_process(pane) }, pane)
end)

return close_pane
