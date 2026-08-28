local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local resize_window = require "utils.resize-window"
local bind_keys = require "utils.bind-keys"
local act = wezterm.action

local M = {}

if platform.is_mac then
  M.SUPER = "CMD"
  M.ALT = "OPT"
  M.SPECIAL = "CMD"
  M.SPECIAL_OR_CTRL = { "CMD", "CTRL" }
  --
  M.SPLITSIDE = "'"
else
  M.SUPER = "CTRL"
  M.ALT = "ALT"
  M.SPECIAL = "ALT"
  M.SPECIAL_OR_CTRL = { "ALT", "CTRL" }
  --
  M.SPLITSIDE = "§"
  --
end

M.attach = wezterm.action_callback(function(_window, pane)
  pane:send_text "mux\n"
end)

---@type Key[]
local keys = bind_keys {
  { key = "c", mods = M.SUPER, action = act.CopyTo "Clipboard" },
  { key = "v", mods = M.SUPER, action = act.PasteFrom "Clipboard" },
  { key = "n", mods = M.SUPER, action = act.SpawnWindow },

  -- Window Management --
  { -- Detach from session
    key = "d",
    mods = M.SPECIAL,
    action = act.DetachDomain "CurrentPaneDomain",
  },
  { -- New Tab --
    key = "t",
    mods = M.SPECIAL_OR_CTRL,
    action = act.SpawnTab "CurrentPaneDomain",
  },
  { -- Quit Window --
    key = "q",
    mods = M.SPECIAL,
    action = act.QuitApplication,
  },
  { -- Quit Tab --
    key = "w",
    mods = M.SPECIAL,
    action = act.CloseCurrentTab { confirm = false },
  },
  { -- Go to last tab --
    key = "0",
    mods = M.SPECIAL,
    action = act.ActivateTab(-1),
  },

  { -- Splt Pane --
    key = M.SPLITSIDE,
    mods = "CMD",
    action = act.SplitHorizontal { domain = "CurrentPaneDomain" },
  },

  -- Zoom Controls --
  { key = "-", mods = M.SUPER, action = resize_window(-50) },
  { key = "+", mods = M.SUPER, action = resize_window(50) },

  -- SSH Archie / Macie
  { key = ".", mods = M.SUPER, action = M.attach },
}

-- Go to tab 1..9
for i = 1, 9 do
  table.insert(keys, {
    key = tostring(i),
    mods = M.SPECIAL,
    action = act.ActivateTab(i - 1),
  })
end

if platform.is_mac then
  ---@type Key[]
  local cursor_motion_keys = {
    { key = "LeftArrow", mods = M.ALT, action = act.SendKey { key = "b", mods = "ALT" } },
    { key = "RightArrow", mods = M.ALT, action = act.SendKey { key = "f", mods = "ALT" } },
    { key = "LeftArrow", mods = M.SUPER, action = act.SendKey { key = "a", mods = "CTRL" } },
    { key = "RightArrow", mods = M.SUPER, action = act.SendKey { key = "e", mods = "CTRL" } },
  }

  table.move(cursor_motion_keys, 1, #cursor_motion_keys, #keys + 1, keys)
end

function M.apply_to_config(config)
  config.disable_default_key_bindings = true
  -- config.disable_default_mouse_bindings = true
  -- config.leader = {}
  config.keys = keys
  -- config.key_tables = key_tables
  -- config.mouse_bindings = mouse_bindings
end

return M
