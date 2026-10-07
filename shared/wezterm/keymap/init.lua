local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local bind_keys = require "utils.keymap.bind-keys"
local keybind = require "utils.keymap.keybind"
local physical_keys = require "keymap.physical-keys"
local motion_keys = require "keymap.motion-keys"
local extend = require "utils.extend"
local mouse_bindings = require "keymap.mouse-bindings"
local skip_close_confirmation = require "utils.skip_close_confirmation"
local close_tab = require "utils.close-tab"
local close_pane = require "utils.close-pane"
local mux = require "utils.mux.mux"
local hwire_session = require "utils.hwire-session"
local MOD = require "keymap.modifiers"
local open_vscode = require "utils.keymap.open-vscode"
local open_jetbrains = require "utils.keymap.open-jetbrains"
local open_github = require "utils.keymap.open-github"
local open_yazi = require "utils.keymap.open-yazi"
local close_window = require "utils.close_window"
local copy_selection = require "utils.copy-selection"
local clear_screen = require "utils.keymap.clear_screen"
local adopt_pane = require "utils.mux.adopt_pane"

local act = wezterm.action

---@type BindKey[]
local bindings = {
  keybind(copy_selection, platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT", "c"),
  keybind(act.PasteFrom "Clipboard", platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT", "v"),
  keybind(act.SpawnWindow, MOD.PRIMARY, "n"),
  keybind(open_yazi, MOD.PRIMARY, "y"),

  ------ Window Management ------

  ---- Application ----
  -- Quit Application --
  keybind(act.QuitApplication, MOD.PRIMARY, { "q", "å" }),


  ---- Window ----
  -- Close Window
  keybind(close_window, MOD.SUPER_REV, "w"),


  ---- Tab ----
  -- Quit Tab --
  keybind(close_tab, MOD.PRIMARY, "w"),

  -- New Tab --
  keybind(hwire_session.new_tab, MOD.PRIMARY, "t"),

  -- New tab in archie/macie --
  keybind(adopt_pane.new_peer_tab, MOD.SUPER_REV, "t"),

  -- Go to last tab --
  keybind(act.ActivateTab(-1), MOD.PRIMARY, "0"),

  -- Go to next/prev tab --
  keybind(act.ActivateTabRelative(1), "CTRL", "Tab"),
  keybind(act.ActivateTabRelative(-1), "CTRL|SHIFT", "Tab"),

  keybind(act.PaneSelect { mode = "MoveToNewTab" }, MOD.PRIMARY, "m"),

  keybind(
    wezterm.action_callback(function(_, pane)
      local tab = pane:move_to_new_tab()
      tab:activate()
    end), MOD.SUPER_REV, "m" ),

  ---- Pane ----
  keybind(close_pane, MOD.UNIQUE, "q"),
  -- Split --
  keybind(act.SplitHorizontal { domain = "CurrentPaneDomain" }, MOD.PRIMARY, "d"),
  keybind(
    act.SplitVertical { domain = "CurrentPaneDomain" },
    platform.is_mac and { "CTRL", MOD.PRIMARY } or MOD.PRIMARY,
    MOD.SPLITBELOW
  ),
  -- Cycle  --
  keybind(act.ActivatePaneDirection "Next", MOD.UNIQUE, "Tab"), -- Forward
  keybind(act.ActivatePaneDirection "Prev", MOD.UNIQUE, "Tab"), -- Backward

  keybind(
    clear_screen,
    platform.is_mac and { MOD.PRIMARY, MOD.SECONDARY, MOD.UNIQUE } or { MOD.PRIMARY, MOD.UNIQUE },
    "l"
  ),

  keybind(adopt_pane.adopt, MOD.SUPER_REV, "a"),
  keybind(act.TogglePaneZoomState, MOD.SUPER_REV, "z"),

  keybind(act.AdjustPaneSize { "Left", 3 }, MOD.SUPER_REV_2, "LeftArrow"),
  keybind(act.AdjustPaneSize { "Right", 3 }, MOD.SUPER_REV_2, "RightArrow"),
  keybind(act.AdjustPaneSize { "Up", 3 }, MOD.SUPER_REV_2, "UpArrow"),
  keybind(act.AdjustPaneSize { "Down", 3 }, MOD.SUPER_REV_2, "DownArrow"),


  ------ Wezterm ------

  keybind("ReloadConfiguration", MOD.SUPER_REV, "r"),

  -- Mux --
  keybind(mux.detach_pane, MOD.SUPER_REV, "s"),
  keybind(adopt_pane.toggle_host, MOD.PRIMARY, "."),

  -- Domain and workspace launcher --
  keybind(act.ShowLauncherArgs { flags = "DOMAINS|WORKSPACES" }, MOD.SUPER_REV, "d"),

  -- Open Vscode --
  keybind(open_vscode, MOD.SUPER_REV, "o"),
  keybind(open_jetbrains.pycharm, MOD.UNIQUE_REV, "p"),
  keybind(open_jetbrains.rustrover, MOD.UNIQUE_REV, "r"),
  keybind(open_jetbrains.intellij, MOD.UNIQUE_REV, "i"),

  keybind(act.ActivateCommandPalette, MOD.PRIMARY, "Space"),
  keybind(act.ShowLauncherArgs { flags = "WORKSPACES" }, MOD.SUPER_REV, "p"),
  keybind(act.ActivateCopyMode, MOD.SUPER_REV, "x"),
  keybind(act.QuickSelect, MOD.SUPER_REV, "Space"),
  keybind(act.SplitVertical { domain = "CurrentPaneDomain" }, MOD.PRIMARY, ";"),
  keybind(open_github, MOD.SUPER_REV, "g"),

}

-- Go to tab 1..9
for i = 1, 9 do
  table.insert(bindings, keybind(act.ActivateTab(i - 1), MOD.PRIMARY, tostring(i)))
end

local keys = bind_keys(bindings)

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
}

return keymap_config
