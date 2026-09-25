local wezterm = require "wezterm"
local MOD = require "keymap.modifiers"
local map = require "keymap.map"
local act = wezterm.action

---@type KeySpec[]
local motion_keys = {
  { -- Back one word
    key = "LeftArrow",
    mods = MOD.UNIQUE,
    action = act.SendString(map.ctrl_left),
    act.SendKey { key = "b", mods = "ALT" },
  },
  { -- Forward one word
    key = "RightArrow",
    mods = MOD.UNIQUE,
    action = act.SendString(map.ctrl_right),
    act.SendKey { key = "f", mods = "ALT" },
  },
  { -- Line start
    key = "LeftArrow",
    mods = MOD.PRIMARY,
    action = act.SendString(map.home),
    act.SendKey { key = "a", mods = "CTRL" },
  },
  { -- Line end
    key = "RightArrow",
    mods = MOD.PRIMARY,
    action = act.SendString(map.line_end),
    act.SendKey { key = "e", mods = "CTRL" },
  },

  { -- Document start
    key = "UpArrow",
    mods = MOD.PRIMARY,
    action = act.ScrollToTop,
  },
  { -- Document end
    key = "DownArrow",
    mods = MOD.PRIMARY,
    action = act.ScrollToBottom,
  },
  { key = "LeftArrow", mods = MOD.SUPER_REV, action = act.SendString(map.shift_home) }, -- Select to line start
  { key = "RightArrow", mods = MOD.SUPER_REV, action = act.SendString(map.shift_end) }, -- Select to line end
  { key = "UpArrow", mods = MOD.SUPER_REV, action = act.SendString(map.ctrl_shift_home) }, -- Select to document start
  { key = "DownArrow", mods = MOD.SUPER_REV, action = act.SendString(map.ctrl_shift_end) }, -- Select to document end

  { key = "UpArrow", mods = MOD.UNIQUE, action = act.ScrollToPrompt(-1) }, -- Previous prompt
  { key = "DownArrow", mods = MOD.UNIQUE, action = act.ScrollToPrompt(1) }, -- Next prompt

  { key = "LeftArrow", mods = "SHIFT", action = act.SendString(map.shift_left) }, -- Select char left
  { key = "RightArrow", mods = "SHIFT", action = act.SendString(map.shift_right) }, -- Select char right
  { key = "UpArrow", mods = "SHIFT", action = act.SendString(map.shift_up) }, -- Select line up
  { key = "DownArrow", mods = "SHIFT", action = act.SendString(map.shift_down) }, -- Select line down
  { key = "LeftArrow", mods = MOD.UNIQUE .. "|SHIFT", action = act.SendString(map.ctrl_shift_left) }, -- Select word left
  { key = "RightArrow", mods = MOD.UNIQUE .. "|SHIFT", action = act.SendString(map.ctrl_shift_right) }, -- Select word right
  { -- Newline without submit
    key = "Enter",
    mods = "SHIFT",
    action = act.SendString(map.shift_enter),
  },
  { -- Open line below
    key = "Enter",
    mods = MOD.PRIMARY,
    action = act.SendString(map.ctrl_e .. map.shift_enter),
  },
  { -- Open line above
    key = "Enter",
    mods = MOD.PRIMARY .. "|SHIFT",
    action = act.SendString(map.ctrl_a .. map.shift_enter .. map.ctrl_b),
  },
  { -- Delete to start
    key = "Backspace",
    mods = MOD.PRIMARY,
    action = act.SendString(map.ctrl_u),
  },
  { -- Delete to end
    key = "Backspace",
    mods = MOD.SUPER_REV,
    action = act.SendString(map.ctrl_k),
  },
}

return motion_keys
