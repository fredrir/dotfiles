local wezterm = require "wezterm"
local MOD = require "keymap.modifiers"
local platform = require "utils.platform"

---@type MouseBinding[]
local mouse_bindings = {
  -- Ctrl or cmd-click will open the link under the mouse cursor
  {
    event = { Up = { streak = 1, button = "Left" } },
    mods = "CTRL",
    action = wezterm.action.OpenLinkAtMouseCursor,
  },
}

if platform.is_mac then
  table.insert(mouse_bindings, {
    event = { Up = { streak = 1, button = "Left" } },
    mods = "CMD",
    action = wezterm.action.OpenLinkAtMouseCursor,
  })
end

return mouse_bindings
