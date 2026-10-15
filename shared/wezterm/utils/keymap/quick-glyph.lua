local wezterm = require "wezterm"
local vtabs = require "plugins.vtabs"

local glyph_search = vtabs.action {
  QuickTerminal = { args = { "zsh", "-lc", "glyph-search" } },
}

return wezterm.action_callback(function(window, pane)
  if pane:get_user_vars().WEZTERM_GIT == "0" then
    return
  end
  window:perform_action(glyph_search, pane)
end)