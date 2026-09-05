local wezterm = require "wezterm"
local MOD = require "keymap.modifiers"
local tmux = require "utils.tmux-workspace"
local act = wezterm.action

local ctrl_a = "\x01"
local ctrl_b = "\x02"
local ctrl_e = "\x05"
local ctrl_u = "\x15"
local ctrl_k = "\x0b"

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
  { key = "UpArrow", mods = MOD.PRIMARY, action = tmux.dispatch("UpArrow", act.ScrollToPrompt(-1)) },
  { key = "DownArrow", mods = MOD.PRIMARY, action = tmux.dispatch("DownArrow", act.ScrollToPrompt(1)) },
  { key = "UpArrow", mods = MOD.SUPER_REV, action = tmux.dispatch("O", act.ActivateCopyMode) },
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
