local wezterm = require "wezterm" ---@type Wezterm
local vtabs = require "plugins.vtabs"

local lazygit = vtabs.action {
  QuickTerminal = { args = { "zsh", "-lc", "$DOTFILES_COMPILED/lazygit" }, width = 0.94, height = 0.94 },
}

return wezterm.action_callback(function(window, pane)
  if pane:get_user_vars().WEZTERM_GIT == "0" then
    return
  end
  window:perform_action(lazygit, pane)
end)
