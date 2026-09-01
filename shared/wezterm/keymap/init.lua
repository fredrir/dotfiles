local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local resize_window = require "utils.resize-window"
local bind_keys = require "utils.bind-keys"
local physical_keys = require "keymap.physical-keys"
local motion_keys = require "keymap.motion-keys"
local extend = require "utils.extend"
local mouse_bindings = require "keymap.mouse-bindings"
local skip_close_confirmation = require "ui.skip_close_confirmation"
local close_tab = require "utils.close-tab"
local close_pane = require "utils.close-pane"
local mux = require "utils.mux"
local hwire_session = require "utils.hwire-session"
local MOD = require "keymap.modifiers"

local act = wezterm.action

---@type Key[]
local keys = bind_keys {
  { key = "c", mods = MOD.PRIMARY, action = act.CopyTo "Clipboard" },
  { key = "v", mods = MOD.PRIMARY, action = act.PasteFrom "Clipboard" },
  { key = "n", mods = MOD.PRIMARY, action = act.SpawnWindow },

  -- Window Management --
  { -- New Tab --
    key = "t",
    mods = MOD.PRIMARY,
    action = hwire_session.new_tab,
  },
  { -- Quit Application --
    key = "q",
    mods = MOD.PRIMARY,
    action = act.QuitApplication,
  },
  { -- Quit Tab (1) --
    key = "w",
    mods = MOD.PRIMARY,
    action = close_tab,
  },
  { -- Go to last tab --
    key = "0",
    mods = MOD.PRIMARY,
    action = act.ActivateTab(-1),
  },
  { -- Go to next tab --
    key = "Tab",
    mods = "CTRL",
    action = act.ActivateTabRelative(1),
  },

  { -- Pane Controls --
    key = "d",
    mods = MOD.PRIMARY,
    action = hwire_session.split "horizontal", -- Split to the side
  },
  {
    key = "q",
    mods = MOD.UNIQUE,
    action = close_pane,
  },
  {
    key = MOD.SPLITBELOW,
    mods = { "CTRL", MOD.PRIMARY },
    action = hwire_session.split "vertical", -- Split Below
  },
  {
    key = "m",
    mods = MOD.PRIMARY,
    action = act.PaneSelect {
      mode = "MoveToNewTab",
    },
  },
  {
    key = "m",
    mods = MOD.SUPER_REV,
    action = wezterm.action_callback(function(_, pane) -- Promote the active split into its own tab
      local tab = pane:move_to_new_tab()
      tab:activate()
    end),
  },
  {
    key = "l",
    mods = { MOD.PRIMARY, MOD.SECONDARY, MOD.UNIQUE }, -- All mod keys + l should clear the screen
    action = act.SendKey {
      key = "l",
      mods = "CTRL",
    },
  },

  -- Zoom Controls --
  { key = "-", mods = MOD.PRIMARY, action = resize_window(-50) },
  { key = "+", mods = MOD.PRIMARY, action = resize_window(50) },

  -- Wezterm ---
  { key = "r", mods = MOD.SUPER_REV, action = "ReloadConfiguration" },

  -- Mux --
  {
    key = "s",
    mods = MOD.SUPER_REV,
    action = mux.detach_pane,
  },
  {
    key = ".",
    mods = MOD.PRIMARY,
    action = mux.attach_detached,
  },
}

-- Go to tab 1..9
for i = 1, 9 do
  table.insert(keys, {
    key = tostring(i),
    mods = MOD.PRIMARY,
    action = act.ActivateTab(i - 1),
  })
end

-- Extenders—
extend(keys, bind_keys(motion_keys))
if platform.is_mac then
  extend(keys, physical_keys)
end

---@type Config
local keymap_config = {
  disable_default_key_bindings = true,
  keys = keys,
  mouse_bindings = mouse_bindings,
  skip_close_confirmation_for_processes_named = skip_close_confirmation,

  -- disable_default_mouse_bindings = true,
  -- leader = {},
  -- key_tables = key_tables,
}

return keymap_config
