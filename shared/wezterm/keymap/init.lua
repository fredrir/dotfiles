local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local resize_window = require "utils.resize-window"
local bind_keys = require "utils.bind-keys"
local macos_no_NB_keys = require "keymap.macos"
local motion_keys = require "keymap.motion-keys"
local extend = require "utils.extend"

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
  { -- Quit Application --
    key = "q",
    mods = MOD.SPECIAL,
    action = act.QuitApplication,
  },
  { -- Quit Tab --
    key = "w",
    mods = MOD.SPECIAL,
    action = act.CloseCurrentTab { confirm = false },
  },
  { -- Quit Pane --
    key = "d",
    mods = MOD.ALT,
    action = act.CloseCurrentPane { confirm = false },
  },
  { -- Go to last tab --
    key = "0",
    mods = MOD.SPECIAL,
    action = act.ActivateTab(-1),
  },
  { -- Go to next tab --
    key = "Tab",
    mods = "CTRL",
    action = act.ActivateTabRelative(1),
  },

  { -- Split Pane to the Side --
    key = "d",
    mods = MOD.SPECIAL,
    action = act.SplitHorizontal { domain = "CurrentPaneDomain" },
  },
  { -- Split Pane Below --
    key = MOD.SPLITBELOW,
    mods = MOD.SPECIAL_OR_CTRL,
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

-- Extenders
extend(keys, motion_keys)
if platform.is_mac then
  extend(keys, macos_no_NB_keys)
end

function M.apply_to_config(config)
  config.disable_default_key_bindings = true
  config.keys = keys
  -- config.disable_default_mouse_bindings = true
  -- config.leader = {}
  -- config.key_tables = key_tables
  -- config.mouse_bindings = mouse_bindings
end

return M
