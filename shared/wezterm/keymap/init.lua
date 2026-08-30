local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local resize_window = require "utils.resize-window"
local bind_keys = require "utils.bind-keys"
local physical_keys = require "keymap.physical-keys"
local motion_keys = require "keymap.motion-keys"
local extend = require "utils.extend"
local mouse_bindings = require "keymap.mouse-bindings"
local skip_close_confirmation = require "ui.skip_close_confirmation"
local mux = require "utils.mux"

local MOD = require "keymap.modifiers"

local act = wezterm.action
local M = {}

---@type Key[]
local keys = bind_keys {
  { key = "c", mods = MOD.SUPER, action = act.CopyTo "Clipboard" },
  { key = "v", mods = MOD.SUPER, action = act.PasteFrom "Clipboard" },
  { key = "n", mods = MOD.SUPER, action = act.SpawnWindow },

  -- Window Management --
  { -- New Tab --
    key = "t",
    mods = MOD.CTRL_OR_SPECIAL,
    action = act.SpawnTab "CurrentPaneDomain",
  },
  { -- Quit Application --
    key = "q",
    mods = MOD.SPECIAL,
    action = act.QuitApplication,
  },
  { -- Quit Tab (1) --
    key = "w",
    mods = MOD.SPECIAL,
    action = act.CloseCurrentTab { confirm = true },
  },
  { -- Go to last tab --
    key = "0",
    mods = MOD.SPECIAL,
    action = act.ActivateTab(-1),
  },
  { -- Go to next tab --
    key = "Tab",
    mods = "CTRL",
    action = act.ActivateTabRelative(1),
  },

  { -- Pane Controls --
    key = "d",
    mods = MOD.SPECIAL,
    action = act.SplitHorizontal { domain = "CurrentPaneDomain" }, -- Split to the side
  },
  {
    key = MOD.SPLITBELOW,
    mods = MOD.CTRL_OR_SPECIAL,
    action = act.SplitVertical { domain = "CurrentPaneDomain" }, -- Split Below
  },

  {
    key = "l",
    mods = MOD.CTRL_OR_SECONDARY_OR_SPECIAL,
    action = act.SendKey {
      key = "l",
      mods = "CTRL",
    },
  },

  -- Zoom Controls --
  { key = "-", mods = MOD.SUPER, action = resize_window(-50) },
  { key = "+", mods = MOD.SUPER, action = resize_window(50) },

  -- Mux --
  {
    key = "s",
    mods = MOD.SUPER_REV,
    action = mux.detach_pane,
  },
  {
    key = ".",
    mods = MOD.SUPER,
    action = mux.attach_pane,
  },
}

-- Go to tab 1..9
for i = 1, 9 do
  table.insert(keys, {
    key = tostring(i),
    mods = MOD.SPECIAL,
    action = act.ActivateTab(i - 1),
  })
end

-- Extenders—
extend(keys, motion_keys)
if platform.is_mac then
  extend(keys, physical_keys)
end

function M.apply_to_config(config)
  config.disable_default_key_bindings = true
  config.keys = keys
  config.mouse_bindings = mouse_bindings
  config.skip_close_confirmation_for_processes_named = skip_close_confirmation

  -- config.disable_default_mouse_bindings = true
  -- config.leader = {}
  -- config.key_tables = key_tables
end

return M
