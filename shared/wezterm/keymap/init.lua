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
local attach_remote = require "utils.attach-remote"
local MOD = require "keymap.modifiers"
local open_vscode = require "utils.open-vscode"
local tmux = require "utils.tmux-workspace"

local act = wezterm.action
local open_yazi = wezterm.action_callback(function(window, pane)
  -- Existing ZLE shells know the original sequence. Tmux needs a reserved
  -- input sequence, then translates it back when writing directly to the shell.
  local sequence = tmux.active(pane) and "\x1b[5;30012~" or "\x1b[115;9u"
  window:perform_action(act.SendString(sequence), pane)
end)
-- Macie's local configuration can disable the vertical-tabs split helper.
local split = hwire_session.split
  or function(direction)
    local command = { domain = "CurrentPaneDomain" }
    return direction == "horizontal" and act.SplitHorizontal(command) or act.SplitVertical(command)
  end

---@type Key[]
local keys = bind_keys {
  { key = "c", mods = platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT", action = act.CopyTo "Clipboard" },
  { key = "v", mods = platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT", action = act.PasteFrom "Clipboard" },
  { key = "n", mods = MOD.PRIMARY, action = act.SpawnWindow },
  { key = "y", mods = MOD.PRIMARY, action = open_yazi },

  -- Window Management --
  { -- New Tab --
    key = "t",
    mods = MOD.PRIMARY,
    action = tmux.dispatch("t", hwire_session.new_tab),
  },
  { -- Quit Application --
    key = "q",
    mods = MOD.PRIMARY,
    action = tmux.dispatch("q", act.QuitApplication),
  },
  { -- Quit Tab (1) --
    key = "w",
    mods = MOD.PRIMARY,
    action = tmux.dispatch("w", close_tab),
  },
  { -- Go to last tab --
    key = "0",
    mods = MOD.PRIMARY,
    action = tmux.dispatch("0", act.ActivateTab(-1)),
  },
  { -- Go to next tab --
    key = "Tab",
    mods = "CTRL",
    action = tmux.dispatch("n", act.ActivateTabRelative(1)),
  },
  {
    key = "Tab",
    mods = "CTRL|SHIFT",
    action = tmux.dispatch("p", act.ActivateTabRelative(-1)),
  },

  { -- Pane Controls --
    key = "d",
    mods = MOD.PRIMARY,
    action = tmux.dispatch("d", split "horizontal"),
  },
  {
    key = "q",
    mods = MOD.UNIQUE,
    action = tmux.dispatch("q", close_pane),
  },
  {
    key = MOD.SPLITBELOW,
    mods = platform.is_mac and { "CTRL", MOD.PRIMARY } or MOD.PRIMARY,
    action = tmux.dispatch("'", split "vertical"),
  },
  {
    key = "m",
    mods = MOD.PRIMARY,
    action = tmux.dispatch(
      "m",
      act.PaneSelect {
        mode = "MoveToNewTab",
      }
    ),
  },
  {
    key = "m",
    mods = MOD.SUPER_REV,
    action = tmux.dispatch(
      "M",
      wezterm.action_callback(function(_, pane)
        local tab = pane:move_to_new_tab()
        tab:activate()
      end)
    ),
  },
  {
    key = "l",
    mods = platform.is_mac and { MOD.PRIMARY, MOD.SECONDARY, MOD.UNIQUE } or { MOD.PRIMARY, MOD.UNIQUE },
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
    action = tmux.dispatch("S", mux.detach_pane),
  },
  {
    key = ".",
    mods = MOD.PRIMARY,
    action = tmux.dispatch(".", mux.attach_detached),
  },
  { -- Attach the best live route to archie/macie --
    key = "k",
    mods = MOD.SUPER_REV,
    action = tmux.dispatch("F7", attach_remote),
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

  { key = "Space", mods = MOD.PRIMARY, action = tmux.dispatch("Space", act.ActivateCommandPalette) },
  { key = "p", mods = MOD.SUPER_REV, action = tmux.dispatch("s", act.ShowLauncherArgs { flags = "WORKSPACES" }) },
  { key = "x", mods = MOD.SUPER_REV, action = tmux.dispatch("Enter", act.ActivateCopyMode) },
  { key = "Space", mods = MOD.SUPER_REV, action = tmux.dispatch("f", act.QuickSelect) },
  { key = ";", mods = MOD.PRIMARY, action = tmux.dispatch("`", split "vertical") },
  { key = "g", mods = MOD.SUPER_REV, action = tmux.dispatch("g", act.Nop) },
  { key = "a", mods = MOD.SUPER_REV, action = tmux.dispatch("a", act.Nop) },
  { key = "z", mods = MOD.SUPER_REV, action = tmux.dispatch("z", act.TogglePaneZoomState) },
  { key = "i", mods = MOD.SUPER_REV, action = tmux.inspect },
  { key = "F12", mods = MOD.PRIMARY, action = tmux.inspect },
  { key = "F12", mods = MOD.SUPER_REV, action = tmux.toggle },
}

-- Go to tab 1..9
for i = 1, 9 do
  table.insert(keys, {
    key = tostring(i),
    mods = MOD.PRIMARY,
    action = tmux.dispatch(tostring(i), act.ActivateTab(i - 1)),
  })
end

-- Directional shortcuts do not take Ctrl-h away from shell history. Inside
-- Neovim, its own Ctrl-h/j/k/l mappings handle editor-to-multiplexer boundaries.
for key, direction in pairs {
  LeftArrow = { "h", "Left" },
  DownArrow = { "j", "Down" },
  UpArrow = { "k", "Up" },
  RightArrow = { "l", "Right" },
} do
  table.insert(keys, {
    key = key,
    mods = platform.is_mac and "CMD|OPT" or "CTRL|ALT",
    action = tmux.dispatch(
      direction[1],
      wezterm.action_callback(function(window, pane)
        local action = pane:get_user_vars().IS_NVIM == "true" and act.SendKey { key = direction[1], mods = "CTRL" }
          or act.ActivatePaneDirection(direction[2])
        window:perform_action(action, pane)
      end)
    ),
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
