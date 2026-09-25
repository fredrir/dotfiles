---@class KeyMap
local map = {
  -- Control characters --
  ctrl_a = "\x01",
  ctrl_b = "\x02",
  ctrl_e = "\x05",
  ctrl_u = "\x15",
  ctrl_k = "\x0b",

  -- Modified navigation --
  ctrl_home = "\x1b[1;5H",
  ctrl_end = "\x1b[1;5F",
  shift_home = "\x1b[1;2H",
  shift_end = "\x1b[1;2F",
  ctrl_shift_home = "\x1b[1;6H",
  ctrl_shift_end = "\x1b[1;6F",

  shift_left = "\x1b[1;2D",
  shift_right = "\x1b[1;2C",
  shift_up = "\x1b[1;2A",
  shift_down = "\x1b[1;2B",
  home = "\x1b[H",
  line_end = "\x1b[F",
  ctrl_left = "\x1b[1;5D",
  ctrl_right = "\x1b[1;5C",
  ctrl_shift_left = "\x1b[1;6D",
  ctrl_shift_right = "\x1b[1;6C",
  shift_enter = "\x1b[13;2u",
}

return map
