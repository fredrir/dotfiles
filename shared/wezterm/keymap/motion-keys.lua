local wezterm = require "wezterm"
local MOD = require "keymap.modifiers"
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

local shift_left = "\x1b[1;2D"
local shift_right = "\x1b[1;2C"
local shift_up = "\x1b[1;2A"
local shift_down = "\x1b[1;2B"
local home = "\x1b[H"
local line_end = "\x1b[F"
local ctrl_left = "\x1b[1;5D"
local ctrl_right = "\x1b[1;5C"
local ctrl_shift_left = "\x1b[1;6D"
local ctrl_shift_right = "\x1b[1;6C"

local alt_b = act.SendKey { key = "b", mods = "ALT" }
local alt_f = act.SendKey { key = "f", mods = "ALT" }
local ctrl_a_key = act.SendKey { key = "a", mods = "CTRL" }
local ctrl_e_key = act.SendKey { key = "e", mods = "CTRL" }

local shift_enter_sequence = "\x1b[13;2u"
local shift_enter = act.SendString(shift_enter_sequence)

local open_line_below = act.SendString(ctrl_e .. shift_enter_sequence)
local open_line_above = act.SendString(ctrl_a .. shift_enter_sequence .. ctrl_b)

local delete_to_start = act.SendString(ctrl_u)
local delete_to_end = act.SendString(ctrl_k)

---@type KeySpec[]
local motion_keys = {
  { key = "LeftArrow", mods = MOD.UNIQUE, action = act.SendString(ctrl_left), alt_b },
  { key = "RightArrow", mods = MOD.UNIQUE, action = act.SendString(ctrl_right), alt_f },
  { key = "LeftArrow", mods = MOD.PRIMARY, action = act.SendString(home), ctrl_a_key },
  { key = "RightArrow", mods = MOD.PRIMARY, action = act.SendString(line_end), ctrl_e_key },

  { key = "UpArrow", mods = MOD.PRIMARY, action = act.SendString(ctrl_home), act.ScrollToTop },
  {
    key = "DownArrow",
    mods = MOD.PRIMARY,
    action = act.SendString(ctrl_end),
    act.ScrollToBottom,
  },
  { key = "LeftArrow", mods = MOD.SUPER_REV, action = act.SendString(shift_home) },
  { key = "RightArrow", mods = MOD.SUPER_REV, action = act.SendString(shift_end) },
  { key = "UpArrow", mods = MOD.SUPER_REV, action = act.SendString(ctrl_shift_home) },
  { key = "DownArrow", mods = MOD.SUPER_REV, action = act.SendString(ctrl_shift_end) },

  { key = "UpArrow", mods = MOD.UNIQUE, action = act.ScrollToPrompt(-1) },
  { key = "DownArrow", mods = MOD.UNIQUE, action = act.ScrollToPrompt(1) },

  { key = "LeftArrow", mods = "SHIFT", action = act.SendString(shift_left) },
  { key = "RightArrow", mods = "SHIFT", action = act.SendString(shift_right) },
  { key = "UpArrow", mods = "SHIFT", action = act.SendString(shift_up) },
  { key = "DownArrow", mods = "SHIFT", action = act.SendString(shift_down) },
  { key = "LeftArrow", mods = MOD.UNIQUE .. "|SHIFT", action = act.SendString(ctrl_shift_left) },
  { key = "RightArrow", mods = MOD.UNIQUE .. "|SHIFT", action = act.SendString(ctrl_shift_right) },
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
