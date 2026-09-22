local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local bind_keys = require "utils.bind-keys"
local physical_keys = require "keymap.physical-keys"
local motion_keys = require "keymap.motion-keys"
local extend = require "utils.extend"
local mouse_bindings = require "keymap.mouse-bindings"
local skip_close_confirmation = require "utils.skip_close_confirmation"
local close_tab = require "utils.close-tab"
local close_pane = require "utils.close-pane"
local mux = require "utils.mux"
local hwire_session = require "utils.hwire-session"
local MOD = require "keymap.modifiers"
local open_vscode = require "utils.open-vscode"
local open_github = require "utils.open-github"
local open_yazi = require "utils.open-yazi"
local close_window = require "utils.close_window"
local copy_selection = require "utils.copy-selection"
require "utils.scrollback"

local act = wezterm.action

local split = hwire_session.split
  or function(direction)
    local command = { domain = "CurrentPaneDomain" }
    return direction == "horizontal" and act.SplitHorizontal(command) or act.SplitVertical(command)
  end

---@type Key[]
local keys = bind_keys {
  { key = "c", mods = platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT", action = copy_selection },
  { key = "v", mods = platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT", action = act.PasteFrom "Clipboard" },
  { key = "n", mods = MOD.PRIMARY, action = act.SpawnWindow },
  { key = "y", mods = MOD.PRIMARY, action = open_yazi },

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
  { -- Quit Tab --
    key = "w",
    mods = MOD.PRIMARY,
    action = close_tab,
  },
  {
    -- Close Window
    key = "w",
    mods = MOD.SUPER_REV,
    action = close_window,
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
  {
    key = "Tab",
    mods = "CTRL|SHIFT",
    action = act.ActivateTabRelative(-1),
  },

  { -- Pane Controls --
    key = "d",
    mods = MOD.PRIMARY,
    action = split "horizontal",
  },
  {
    key = "q",
    mods = MOD.UNIQUE,
    action = close_pane,
  },
  {
    key = MOD.SPLITBELOW,
    mods = platform.is_mac and { "CTRL", MOD.PRIMARY } or MOD.PRIMARY,
    action = split "vertical",
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
    action = wezterm.action_callback(function(_, pane)
      local tab = pane:move_to_new_tab()
      tab:activate()
    end),
  },
  {
    key = "l",
    mods = platform.is_mac and { MOD.PRIMARY, MOD.SECONDARY, MOD.UNIQUE } or { MOD.PRIMARY, MOD.UNIQUE },
    action = act.SendKey {
      key = "l",
      mods = "CTRL",
    },
  },
  { key = "LeftArrow", mods = MOD.SUPER_REV_2, action = act.AdjustPaneSize { "Left", 3 } },
  { key = "RightArrow", mods = MOD.SUPER_REV_2, action = act.AdjustPaneSize { "Right", 3 } },
  { key = "UpArrow", mods = MOD.SUPER_REV_2, action = act.AdjustPaneSize { "Up", 3 } },
  { key = "DownArrow", mods = MOD.SUPER_REV_2, action = act.AdjustPaneSize { "Down", 3 } },

  -- Wezterm ---
  { key = "r", mods = MOD.SUPER_REV, action = "ReloadConfiguration" },

  -- Mux --
  {
    key = "s",
    mods = MOD.SUPER_REV,
    action = mux.detach_pane,
  },
  { -- Domain and workspace launcher --
    key = "d",
    mods = MOD.SUPER_REV,
    action = act.ShowLauncherArgs { flags = "DOMAINS|WORKSPACES" },
  },
  { -- Open Vscode --
    key = "o",
    mods = MOD.SUPER_REV,
    action = open_vscode,
  },

  { key = "Space", mods = MOD.PRIMARY, action = act.ActivateCommandPalette },
  { key = "p", mods = MOD.SUPER_REV, action = act.ShowLauncherArgs { flags = "WORKSPACES" } },
  { key = "x", mods = MOD.SUPER_REV, action = act.ActivateCopyMode },
  { key = "Space", mods = MOD.SUPER_REV, action = act.QuickSelect },
  { key = ";", mods = MOD.PRIMARY, action = split "vertical" },
  {
    key = "g",
    mods = MOD.SUPER_REV,
    action = open_github,
  },
  { key = "a", mods = MOD.SUPER_REV, action = act.Nop },
  { key = "z", mods = MOD.SUPER_REV, action = act.TogglePaneZoomState },
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
