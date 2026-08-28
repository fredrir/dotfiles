local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local act = wezterm.action

local M = {}

if platform.is_mac then
  M.SUPER = "CMD"
  M.MUX = "CMD|SHIFT"
elseif platform.is_win or platform.is_linux then
  M.SUPER = "CTRL"
  M.MUX = "ALT|SHIFT"
end

---@type Key[]
local keys = {
  { key = "c", mods = M.SUPER, action = act.CopyTo "Clipboard" },
  { key = "v", mods = M.SUPER, action = act.PasteFrom "Clipboard" },
  { key = "n", mods = M.SUPER, action = act.SpawnWindow },

  -- window: zoom window
  {
    key = "-",
    mods = M.SUPER,
    action = wezterm.action_callback(function(window, _pane)
      local dimensions = window:get_dimensions()
      local new_width = dimensions.pixel_width - 50
      local new_height = dimensions.pixel_height - 50
      window:set_inner_size(new_width, new_height)
    end),
  },
  {
    key = "+",
    mods = M.SUPER,
    action = wezterm.action_callback(function(window, _pane)
      local dimensions = window:get_dimensions()
      local new_width = dimensions.pixel_width + 50
      local new_height = dimensions.pixel_height + 50
      window:set_inner_size(new_width, new_height)
    end),
  },

  -- SSH Archie / Macie
  { key = ".", mods = M.SUPER },
}

---@type Config
return {
  disable_default_key_bindings = true,
  -- disable_default_mouse_bindings = true,
  leader = { key = "Space", mods = M.MUX },
  keys = keys,
  -- key_tables = key_tables,
  -- mouse_bindings = mouse_bindings,
}
