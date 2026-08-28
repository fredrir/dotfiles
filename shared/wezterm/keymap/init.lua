local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"

local M = {}

if platform.is_mac then
  M.SUPER = "SUPER"
  M.SUPER_REV = "SUPER|CTRL"
elseif platform.is_win or platform.is_linux then
  M.SUPER = "Ctrl"
  M.SUPER_REV = "ALT|CTRL"
end

---@type Key[]
local keys = {
  { key = "c", mods = "CTRL|SHIFT", action = act.CopyTo "Clipboard" },
  { key = ".", mods = "ALT" },
}

M.attach = wezterm.action_callback(function(_, pane)
  pane:send_text "mux\n"
end)

function M.apply_to_config(config)
  local platform = wezterm.target_triple:find "darwin" and "keymap.macos" or "keymap.linux"
  local chord = require(platform)

  config.keys = {
    { key = chord.key, mods = chord.mods, action = M.attach },
  }
end

return M
