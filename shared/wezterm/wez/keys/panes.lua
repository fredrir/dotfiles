local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

if os.getenv 'DMUX_WEZ_FIRST' == '1' then
  -- Native Split actions bypass owner/epoch validation. The mandatory final
  -- dmux layer installs their backend-aware replacements.
  M.keys = {}
  M.key_tables = {}
  return M
end

-- Punctuation is bound by physical key: on the Norwegian layout the character
-- a key produces differs from US, and a char binding would silently never fire.
M.keys = {
  { key = 'phys:Backslash', mods = 'LEADER', action = act.SplitHorizontal { domain = 'CurrentPaneDomain' } },
  { key = 'phys:Minus', mods = 'LEADER', action = act.SplitVertical { domain = 'CurrentPaneDomain' } },
  { key = 'phys:8', mods = 'CTRL|SHIFT', action = act.SplitHorizontal { domain = 'CurrentPaneDomain' } },
  { key = 'phys:9', mods = 'CTRL|SHIFT', action = act.SplitVertical { domain = 'CurrentPaneDomain' } },

  { key = 'h', mods = 'LEADER', action = act.ActivatePaneDirection 'Left' },
  { key = 'j', mods = 'LEADER', action = act.ActivatePaneDirection 'Down' },
  { key = 'k', mods = 'LEADER', action = act.ActivatePaneDirection 'Up' },
  { key = 'l', mods = 'LEADER', action = act.ActivatePaneDirection 'Right' },

  { key = 'z', mods = 'LEADER', action = act.TogglePaneZoomState },
  { key = 'x', mods = 'LEADER', action = act.CloseCurrentPane { confirm = true } },

  {
    key = 'r',
    mods = 'LEADER',
    action = act.ActivateKeyTable { name = 'resize_pane', one_shot = false },
  },
}

M.key_tables = {
  resize_pane = {
    { key = 'h', action = act.AdjustPaneSize { 'Left', 3 } },
    { key = 'j', action = act.AdjustPaneSize { 'Down', 3 } },
    { key = 'k', action = act.AdjustPaneSize { 'Up', 3 } },
    { key = 'l', action = act.AdjustPaneSize { 'Right', 3 } },
    { key = 'Escape', action = act.PopKeyTable },
    { key = 'Enter', action = act.PopKeyTable },
  },
}

return M
