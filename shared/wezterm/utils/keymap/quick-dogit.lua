local wezterm = require "wezterm" ---@type Wezterm
local vtabs = require "plugins.vtabs"

---@param command string
---@return Action
local function action(command)
  local quick_dogit = vtabs.action {
    QuickTerminal = { args = { "zsh", "-lc", command }, width = 0.94, height = 0.94 },
  }

  return wezterm.action_callback(function(window, pane)
    if pane:get_user_vars().WEZTERM_GIT == "0" then
      return
    end
    window:perform_action(quick_dogit, pane)
  end)
end

return {
  open = action "\"$DOTFILES_COMPILED/dogit\"",
  log = action "\"$DOTFILES_COMPILED/dogit\" log",
}