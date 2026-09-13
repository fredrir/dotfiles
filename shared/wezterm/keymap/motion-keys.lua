local wezterm = require "wezterm"
local MOD = require "keymap.modifiers"
local tmux = require "utils.tmux-workspace"
local act = wezterm.action

local ctrl_a = "\x01"
local ctrl_b = "\x02"
local ctrl_e = "\x05"
local ctrl_u = "\x15"
local ctrl_k = "\x0b"

local ctrl_home = "\x1b[1;5H"
local ctrl_end = "\x1b[1;5F"
local shift_home = "\x1b[1;2H"
local shift_end = "\x1b[1;2F"
local ctrl_shift_home = "\x1b[1;6H"
local ctrl_shift_end = "\x1b[1;6F"

local shift_enter_sequence = "\x1b[13;2u"
local shift_enter = act.SendString(shift_enter_sequence)

local open_line_below = act.SendString(ctrl_e .. shift_enter_sequence)
local open_line_above = wezterm.action_callback(function(window, pane)
  -- Ctrl-b is cursor-left in ZLE and also the tmux prefix. Forward it through
  -- the prefix binding so this editing gesture leaves tmux in its root table.
  local cursor_left = tmux.active(pane) and ctrl_b .. ctrl_b or ctrl_b
  window:perform_action(act.SendString(ctrl_a .. shift_enter_sequence .. cursor_left), pane)
end)

local delete_to_start = act.SendString(ctrl_u)
local delete_to_end = act.SendString(ctrl_k)

---@type KeySpec[]
local motion_keys = {
  { key = "LeftArrow", mods = MOD.UNIQUE, action = act.SendKey { key = "b", mods = "ALT" } },
  { key = "RightArrow", mods = MOD.UNIQUE, action = act.SendKey { key = "f", mods = "ALT" } },
  { key = "LeftArrow", mods = MOD.PRIMARY, action = act.SendKey { key = "a", mods = "CTRL" } },
  { key = "RightArrow", mods = MOD.PRIMARY, action = act.SendKey { key = "e", mods = "CTRL" } },

  -- Document and selection motions travel as the sequences an editor already
  -- understands, so Neovim keeps working while ZLE gains the macOS gestures.
  { key = "UpArrow", mods = MOD.PRIMARY, action = act.SendString(ctrl_home) },
  { key = "DownArrow", mods = MOD.PRIMARY, action = act.SendString(ctrl_end) },
  { key = "LeftArrow", mods = MOD.SUPER_REV, action = act.SendString(shift_home) },
  { key = "RightArrow", mods = MOD.SUPER_REV, action = act.SendString(shift_end) },
  { key = "UpArrow", mods = MOD.SUPER_REV, action = act.SendString(ctrl_shift_home) },
  { key = "DownArrow", mods = MOD.SUPER_REV, action = act.SendString(ctrl_shift_end) },

  -- Prompt jumping moves off Cmd, which the document motions now own.
  { key = "UpArrow", mods = MOD.UNIQUE, action = tmux.dispatch("Up", act.ScrollToPrompt(-1)) },
  { key = "DownArrow", mods = MOD.UNIQUE, action = tmux.dispatch("Down", act.ScrollToPrompt(1)) },
  {
    key = "Enter",
    mods = "SHIFT",
    action = shift_enter,
  },
  {
    key = "Enter",
    mods = MOD.PRIMARY,
    action = open_line_below,
  },
  {
    key = "Enter",
    mods = MOD.PRIMARY .. "|SHIFT",
    action = open_line_above,
  },
  {
    key = "Backspace",
    mods = MOD.PRIMARY,
    action = delete_to_start,
  },
  {
    key = "Backspace",
    mods = MOD.SUPER_REV,
    action = delete_to_end,
  },
}

return motion_keys
