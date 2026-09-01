local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action

---@param tab MuxTab
---@return boolean
local function has_running_process(tab)
  for _, info in ipairs(tab:panes_with_info()) do
    if info.pane:get_user_vars().WEZTERM_PROG ~= "" then
      return true
    end
  end

  return false
end

local close_tab = wezterm.action_callback(function(window, pane)
  window:perform_action(act.CloseCurrentTab { confirm = has_running_process(window:active_tab()) }, pane)
end)

return close_tab
