local wezterm = require "wezterm" ---@type Wezterm
local adopt_pane = require "utils.mux.adopt_pane"
local host = require "domain.hosts"

---@param target string
local function send(window, pane, target)
  window:perform_action(wezterm.action.SendString("\x1b[5;30020~" .. target .. "\a"), pane)
end

---@param target string
---@return RevAction
return function(target)
  return {
    action = wezterm.action_callback(function(window, pane)
      if pane:get_user_vars().WEZTERM_PROG == "" then
        send(window, pane, target)
      end
    end),
    rev = wezterm.action_callback(function(window, pane)
      local user_vars = pane:get_user_vars()
      if user_vars.WEZTERM_PROG ~= "" then
        return
      end
      if user_vars.WEZTERM_HOST == host.target.hostname then
        send(window, pane, target)
      else
        adopt_pane.go_to_peer(window, pane, target)
      end
    end),
  }
end
