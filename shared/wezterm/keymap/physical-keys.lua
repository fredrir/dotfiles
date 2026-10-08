local wezterm = require "wezterm"
local extend = require "utils.extend"
local act = wezterm.action
local platform = require "utils.platform"

---@type KeySpec[]
local physical_keys = {}

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
  extend(physical_keys, { -- CMD+H on mac should act the same as CTRL+R
    {
      key = "phys:h",
      mods = "CMD",
      action = act.SendKey {
        key = "h",
        mods = "CTRL",
      },
    },
  })
end

return physical_keys