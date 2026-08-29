local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local resize_window = require "utils.resize-window"
local bind_keys = require "utils.bind-keys"
local act = wezterm.action
local MOD = require "keymap.modifiers"

local M = {}

M.attach = wezterm.action_callback(function(_window, pane)
  pane:send_text "mux\n"
end)

---@type Key[]
local keys = bind_keys {
  { key = "c", mods = MOD.SUPER, action = act.CopyTo "Clipboard" },
  { key = "v", mods = MOD.SUPER, action = act.PasteFrom "Clipboard" },
  { key = "n", mods = MOD.SUPER, action = act.SpawnWindow },

  -- Window Management --
  { -- New Tab --
    key = "t",
    mods = MOD.SPECIAL_OR_CTRL,
    action = act.SpawnTab "CurrentPaneDomain",
  },
  -- { -- Detach Tab
  --   key = "s",
  --   mods = MOD.SUPER_REV,
  --   action = act.DetachTab "CurrentTabDomain",
  -- },
  { -- Quit Window --
    key = "q",
    mods = MOD.SPECIAL,
    action = act.QuitApplication,
  },
  { -- Quit Tab --
    key = "w",
    mods = MOD.SPECIAL,
    action = act.CloseCurrentTab { confirm = false },
  },
  { -- Go to last tab --
    key = "0",
    mods = MOD.SPECIAL,
    action = act.ActivateTab(-1),
  },

  { -- Split Pane to the Side --
    key = MOD.SPLITSIDE,
    mods = "CMD",
    action = act.SplitHorizontal { domain = "CurrentPaneDomain" },
  },
  { -- Split Pane Below --
    key = MOD.SPECIAL,
    mods = "CMD",
    action = act.SplitVertical { domain = "CurrentPaneDomain" },
  },

  -- Zoom Controls --
  { key = "-", mods = MOD.SUPER, action = resize_window(-50) },
  { key = "+", mods = MOD.SUPER, action = resize_window(50) },

  -- SSH Archie / Macie
  { key = ".", mods = MOD.SUPER, action = M.attach },
}

-- Go to tab 1..9
for i = 1, 9 do
  table.insert(keys, {
    key = tostring(i),
    mods = MOD.SPECIAL,
    action = act.ActivateTab(i - 1),
  })
end

if platform.is_mac then
  ---@type Key[]
  local cursor_motion_keys = {
    { key = "LeftArrow", mods = MOD.ALT, action = act.SendKey { key = "b", mods = "ALT" } },
    { key = "RightArrow", mods = MOD.ALT, action = act.SendKey { key = "f", mods = "ALT" } },
    { key = "LeftArrow", mods = MOD.SUPER, action = act.SendKey { key = "a", mods = "CTRL" } },
    { key = "RightArrow", mods = MOD.SUPER, action = act.SendKey { key = "e", mods = "CTRL" } },
  }

  -- Fixes ⌥+7, ⌥+8 --> [ , ]

  table.move(cursor_motion_keys, 1, #cursor_motion_keys, #keys + 1, keys)
end

function M.apply_to_config(config)
  config.disable_default_key_bindings = true
  config.keys = keys

  config.keys = keys
  -- config.disable_default_mouse_bindings = true
  -- config.leader = {}
  -- config.key_tables = key_tables
  -- config.mouse_bindings = mouse_bindings
end

return M
