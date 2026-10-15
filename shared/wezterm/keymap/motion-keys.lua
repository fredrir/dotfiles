local wezterm = require "wezterm"
local MOD = require "keymap.modifiers"
local map = require "keymap.map"
local act = wezterm.action

---@param key string
---@param scroll_action Action
---@return Action
local function document_edge(key, scroll_action)
  return wezterm.action_callback(function(window, pane)
    local action = pane:is_alt_screen_active() and act.SendKey { key = key } or scroll_action
    window:perform_action(action, pane)
  end)
end

---@class MotionKey: KeySpec
---@field free_cursor? CopyModeParams

---@type MotionKey[]
local motion_keys = {
  { -- Back one word
    key = "LeftArrow",
    mods = MOD.UNIQUE,
    action = act.SendString(map.ctrl_left),
    free_cursor = "MoveBackwardWord",
    act.SendKey { key = "b", mods = "ALT" },
  },
  { -- Forward one word
    key = "RightArrow",
    mods = MOD.UNIQUE,
    action = act.SendString(map.ctrl_right),
    free_cursor = "MoveForwardWord",
    act.SendKey { key = "f", mods = "ALT" },
  },
  { -- Line start
    key = "LeftArrow",
    mods = MOD.PRIMARY,
    action = act.SendString(map.home),
    free_cursor = "MoveToStartOfLine",
    act.SendKey { key = "a", mods = "CTRL" },
  },
  { -- Line end
    key = "RightArrow",
    mods = MOD.PRIMARY,
    action = act.SendString(map.line_end),
    free_cursor = "MoveToEndOfLineContent",
    act.SendKey { key = "e", mods = "CTRL" },
  },

  { -- Document start
    key = "UpArrow",
    mods = MOD.PRIMARY,
    action = document_edge("Home", act.ScrollToTop),
    free_cursor = "MoveToScrollbackTop",
  },
  { -- Document end
    key = "DownArrow",
    mods = MOD.PRIMARY,
    action = document_edge("End", act.ScrollToBottom),
    free_cursor = "MoveToScrollbackBottom",
  },
  { -- Select to line start
    key = "LeftArrow",
    mods = MOD.SUPER_REV,
    action = act.SendString(map.shift_home),
    free_cursor = "MoveToStartOfLine",
  },
  { -- Select to line end
    key = "RightArrow",
    mods = MOD.SUPER_REV,
    action = act.SendString(map.shift_end),
    free_cursor = "MoveToEndOfLineContent",
  },
  { -- Select to document start
    key = "UpArrow",
    mods = MOD.SUPER_REV,
    action = act.SendString(map.ctrl_shift_home),
    free_cursor = "MoveToScrollbackTop",
  },
  { -- Select to document end
    key = "DownArrow",
    mods = MOD.SUPER_REV,
    action = act.SendString(map.ctrl_shift_end),
    free_cursor = "MoveToScrollbackBottom",
  },

  { -- Previous prompt
    key = "UpArrow",
    mods = MOD.UNIQUE,
    action = act.ScrollToPrompt(-1),
    free_cursor = { MoveBackwardZoneOfType = "Prompt" },
  },
  { -- Next prompt
    key = "DownArrow",
    mods = MOD.UNIQUE,
    action = act.ScrollToPrompt(1),
    free_cursor = { MoveForwardZoneOfType = "Prompt" },
  },

  { -- Select char left
    key = "LeftArrow",
    mods = "SHIFT",
    action = act.SendString(map.shift_left),
    free_cursor = "MoveLeft",
  },
  { -- Select char right
    key = "RightArrow",
    mods = "SHIFT",
    action = act.SendString(map.shift_right),
    free_cursor = "MoveRight",
  },
  { -- Select line up
    key = "UpArrow",
    mods = "SHIFT",
    action = act.SendString(map.shift_up),
    free_cursor = "MoveUp",
  },
  { -- Select line down
    key = "DownArrow",
    mods = "SHIFT",
    action = act.SendString(map.shift_down),
    free_cursor = "MoveDown",
  },
  { -- Select word left
    key = "LeftArrow",
    mods = MOD.UNIQUE .. "|SHIFT",
    action = act.SendString(map.ctrl_shift_left),
    free_cursor = "MoveBackwardWord",
  },
  { -- Select word right
    key = "RightArrow",
    mods = MOD.UNIQUE .. "|SHIFT",
    action = act.SendString(map.ctrl_shift_right),
    free_cursor = "MoveForwardWord",
  },
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