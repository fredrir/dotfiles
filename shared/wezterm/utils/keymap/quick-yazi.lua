local wezterm = require "wezterm"
local vtabs = require "plugins.vtabs"
local open_vscode = require "utils.keymap.open-vscode"

local quick_yazi = vtabs.action {
  QuickTerminal = { args = { "zsh", "-lc", "yazi" }, set_environment_variables = { YAZI_OPEN_VSCODE = "1" } },
}

wezterm.on("user-var-changed", function(window, pane, name, value)
  if name ~= "OPEN_VSCODE" or value == "" then
    return
  end

  open_vscode.open(value, pane)
  -- toggling the shown program hides it and ends yazi
  window:perform_action(quick_yazi, pane)
end)

return quick_yazi
