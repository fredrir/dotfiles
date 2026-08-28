local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local act = wezterm.action
local resize_window = require "utils.resize-window"

local M = {}

if platform.is_mac then
  M.SUPER = "CMD"
  M.MOD = "CMD"
  M.ALT = "OPT"
  M.MUX = "CMD|SHIFT"
elseif platform.is_win or platform.is_linux then
  M.SUPER = "CTRL"
  M.MOD = "Alt"
  M.ALT = "ALT"
  M.MUX = "ALT|SHIFT"
end

M.attach = wezterm.action_callback(function(_window, pane)
  pane:send_text "mux\n"
end)

---@type Key[]
local keys = {
  { key = "c", mods = M.SUPER, action = act.CopyTo "Clipboard" },
  { key = "v", mods = M.SUPER, action = act.PasteFrom "Clipboard" },
  { key = "n", mods = M.SUPER, action = act.SpawnWindow },

  -- Window Management --
  { key = "-", mods = M.SUPER, action = resize_window(-50) },
  { key = "+", mods = M.SUPER, action = resize_window(50) },

  -- SSH Archie / Macie
  { key = ".", mods = M.SUPER, action = M.attach },
}

---@type Key[]
local mac_cursor_motion_keys = {
  { key = "LeftArrow", mods = "ALT", action = act.SendKey { key = "b", mods = "ALT" } },
  { key = "RightArrow", mods = "ALT", action = act.SendKey { key = "f", mods = "ALT" } },
  { key = "LeftArrow", mods = "CMD", action = act.SendKey { key = "a", mods = "CTRL" } },
  { key = "RightArrow", mods = "CMD", action = act.SendKey { key = "e", mods = "CTRL" } },
}

-- if platform.is_mac then
--   for _, key in ipairs(mac_cursor_motion_keys) do
--     table.insert(keys, key)
--   end
-- end

---@type Config
return {
  disable_default_key_bindings = true,
  -- disable_default_mouse_bindings = true,
  leader = { key = "SHIFT", mods = M.MOD },
  keys = keys,
  -- key_tables = key_tables,
  -- mouse_bindings = mouse_bindings,
}
