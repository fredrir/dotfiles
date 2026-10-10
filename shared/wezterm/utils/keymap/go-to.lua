local wezterm = require "wezterm" ---@type Wezterm

---@param target string
return function(target)
  return wezterm.action_callback(function(window, pane)
    if pane:get_user_vars().WEZTERM_PROG == "" then
      window:perform_action(wezterm.action.SendString("\x1b[5;30020~" .. target .. "\a"), pane)
    end
  end)
end