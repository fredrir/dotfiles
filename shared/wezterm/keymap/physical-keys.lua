local wezterm = require "wezterm"
local extend = require "utils.extend"
local act = wezterm.action
local platform = require "utils.platform"

---@type KeySpec[]
local physical_keys = {
  -- Fixes ⌥+7 '[', ⌥+8 ']',
  { key = "phys:8", mods = "OPT", action = act.SendString "[" },
  { key = "phys:9", mods = "OPT", action = act.SendString "]" },

  { key = "phys:8", mods = "OPT|SHIFT", action = act.SendString "{" },
  { key = "phys:9", mods = "OPT|SHIFT", action = act.SendString "}" },

  { key = "phys:7", mods = "OPT|SHIFT", action = act.SendString "\\" },
}

if platform.is_mac then
  extend(physical_keys, { -- CMD+R on mac should act the same as CTRL+R
    {
      key = "phys:r",
      mods = "CMD",
      action = act.SendKey {
        key = "r",
        mods = "CTRL",
      },
    },
  })
end

return physical_keys
