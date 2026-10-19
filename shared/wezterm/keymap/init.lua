local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local bind_keys = require "utils.keymap.bind-keys"
local keybind = require "utils.keymap.keybind"
local physical_keys = require "keymap.physical-keys"
local motion_keys = require "keymap.motion-keys"
local free_cursor = require "utils.keymap.free-cursor"
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
local git_add = require "utils.keymap.git-add"
local open_yazi = require "utils.keymap.open-yazi"
local quick_yazi = require "utils.keymap.quick-yazi"
local quick_dogit = require "utils.keymap.quick-dogit"
local go_to = require "utils.keymap.go-to"
local cd_git_root = require "utils.keymap.cd-git-root"
local close_window = require "utils.close_window"
local copy_selection = require "utils.copy-selection"
local delete_selection = require "utils.delete-selection"
local clear_screen = require "utils.keymap.clear_screen"
local adopt_pane = require "utils.mux.adopt_pane"
local resize_window = require "utils.resize-window"
local quick_glyph = require "utils.keymap.quick-glyph"

local act = wezterm.action

---@type BindKey[]
local bindings = {
  keybind(delete_selection, "NONE", "Backspace"),
  keybind(copy_selection, platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT", "c"),
  keybind(act.PasteFrom "Clipboard", platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT", "v"),
  keybind(free_cursor.activate, platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT", "e"),
  keybind(act.SpawnWindow, MOD.PRIMARY, "n"),
  keybind(open_yazi, MOD.PRIMARY, "y"),

  ------ Window Management ------

  ---- Application ----
  -- Quit Application --
  keybind(act.QuitApplication, MOD.PRIMARY, "q"),

  ---- Window ----
  -- Close Window
  keybind(close_window, MOD.SUPER_REV, "w"),

  ---- Tab ----
  -- Quit Tab --
  keybind(close_tab, MOD.PRIMARY, "w"),

  -- New Tab --
  keybind(hwire_session.new_tab, MOD.PRIMARY, "t"),

  -- New Tab in archie/macie --
  keybind(adopt_pane.new_peer_tab, MOD.SUPER_REV, "t"),

  -- Go to last Tab --
  keybind(act.ActivateTab(-1), MOD.PRIMARY, "0"),

  -- Go to next/prev Tab --
  keybind(act.ActivateTabRelative(1), { "CTRL" }, "Tab"),
  keybind(act.ActivateTabRelative(-1), { "CTRL|SHIFT", MOD.SHIFT_RIGHT }, "Tab"),

  keybind(act.PaneSelect { mode = "MoveToNewTab" }, MOD.PRIMARY, "m"),

  keybind(
    wezterm.action_callback(function(_, pane)
      local tab = pane:move_to_new_tab()
      tab:activate()
    end),
    MOD.SUPER_REV,
    "m"
  ),

  ---- Pane ----
  keybind(close_pane, MOD.UNIQUE, { "q", "d" }),
  -- Split --
  keybind(act.SplitHorizontal { domain = "CurrentPaneDomain" }, MOD.PRIMARY, "d"),
  keybind(
    act.SplitVertical { domain = "CurrentPaneDomain" },
    platform.is_mac and { "CTRL", MOD.PRIMARY } or MOD.PRIMARY,
    MOD.SPLITBELOW
  ),
  keybind(act.SplitVertical { domain = "CurrentPaneDomain" }, MOD.PRIMARY, ";"),
  -- Cycle  --
  keybind(act.ActivatePaneDirection "Next", MOD.UNIQUE, "Tab"), -- Forward
  keybind(act.ActivatePaneDirection "Prev", MOD.UNIQUE, "Tab"), -- Backward

  keybind(clear_screen, { "CTRL", MOD.PRIMARY, MOD.UNIQUE }, "l"),

  keybind(act.TogglePaneZoomState, MOD.SUPER_REV, "z"),

  keybind(resize_window(true), MOD.PRIMARY, "+"),
  keybind(resize_window(false), MOD.PRIMARY, "-"),

  ------ Wezterm ------

  keybind("ReloadConfiguration", MOD.SUPER_REV, "r"),

  -- Font size --
  keybind(act.IncreaseFontSize, MOD.SUPER_REV, "+"),
  keybind(act.DecreaseFontSize, MOD.SUPER_REV, "-"),
  keybind(act.ResetFontSize, MOD.UNIQUE_REV, "?"),

  -- Mux --
  keybind(mux.detach_pane, MOD.SUPER_REV, "s"),
  keybind(adopt_pane.toggle_host, { MOD.PRIMARY, "CTRL" }, "."),
  keybind(adopt_pane.toggle_host, MOD.UNIQUE, "Enter"),

  -- Open Editors --
  keybind(open_vscode.cwd, MOD.SUPER_REV, "o"),
  keybind(open_jetbrains.pycharm, MOD.SUPER_REV, "p"),
  keybind(open_jetbrains.rustrover, MOD.UNIQUE_REV, "r"),
  keybind(open_github, MOD.UNIQUE_REV, "g"),

  keybind(act.QuickSelect, MOD.SUPER_REV, "Space"),

  ------ Directory Navigation ------
  ----
  keybind(go_to "HOME", { MOD.SHIFT_RIGHT, MOD.UNIQUE }, "h"),
  keybind(go_to "HOME/.agents/skills", { MOD.UNIQUE }, "a"),
  keybind(go_to "HOME/.claude/skills", { MOD.UNIQUE }, "c"),
  ----
  keybind(go_to "DOTFILES/shared/wezterm", { MOD.SHIFT_RIGHT, MOD.UNIQUE }, "w"),
  ----
  keybind(go_to "PYPARSER", { MOD.UNIQUE }, "p"),
  keybind(go_to "LLUNDE", MOD.SUPER_REV, "l"),
  ----
  keybind(go_to "CONFIG", MOD.SHIFT_RIGHT, "c"),
  keybind(go_to "DOTFILES", MOD.SHIFT_RIGHT, "d"),
  keybind(go_to "DOTFILES/scripts/rust/crates/git/dogit", MOD.SHIFT_RIGHT, "x"),
  keybind(go_to "HOME/infra", MOD.SHIFT_RIGHT, "i"),
  keybind(cd_git_root, MOD.SHIFT_RIGHT, "g"),
  keybind(go_to "OLDPWD", MOD.SHIFT_RIGHT, "j"),
  keybind(go_to "PYPARSER", { MOD.SHIFT_RIGHT }, "q"),
  keybind(go_to "DOTFILES/shared/zsh", MOD.SHIFT_RIGHT, "z"),
  keybind(go_to "HOME/rif", MOD.SHIFT_RIGHT, "r"),
  keybind(go_to "HOME/ArchTeX", { MOD.SHIFT_RIGHT }, "a"),

  keybind(go_to "HOME/projects", MOD.SHIFT_RIGHT, "p"),
  keybind(go_to "HOME/projects/obsidian", MOD.SHIFT_RIGHT, "o"),
  keybind(go_to "HOME/projects/shucked", MOD.SHIFT_RIGHT, "s"),
  keybind(go_to "HOME/projects/wez-plugins/vertical-tabs", MOD.SHIFT_RIGHT, "v"),

  ------ Quick Actions ------
  ----
  keybind(quick_dogit.open, MOD.PRIMARY, "g"),
  keybind(quick_dogit.log, MOD.SUPER_REV, "g"),
  keybind(quick_yazi, MOD.PRIMARY, "y"),
  keybind(quick_glyph, MOD.UNIQUE, "g"),
  ----
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
  key_tables = { copy_mode = free_cursor.key_table(keys) },
  mouse_bindings = mouse_bindings,
  skip_close_confirmation_for_processes_named = skip_close_confirmation,
  use_dead_keys = false,
  send_composed_key_when_left_alt_is_pressed = platform.is_mac,
}

return keymap_config