local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action

-- A bare WezTerm pane has no shell-side scrollback client, so ZLE reports the
-- intent and WezTerm performs the scroll.
local actions = {
  top = act.ScrollToTop,
  bottom = act.ScrollToBottom,
}

wezterm.on("user-var-changed", function(window, pane, name, value)
  if name ~= "SCROLLBACK" then
    return
  end
  local action = actions[value]
  if action then
    window:perform_action(action, pane)
  end
end)

return actions
