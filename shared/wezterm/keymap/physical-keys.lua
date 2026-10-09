local wezterm = require "wezterm"
local extend = require "utils.extend"
local act = wezterm.action
local platform = require "utils.platform"

---@type KeySpec[]
local physical_keys = {}

if platform.is_mac then
  -- Norwegian accent keys: send literal symbols before macOS dead-key composition.
  -- Equal is the ´ key; RightBracket is the ¨ key on this layout.
  extend(physical_keys, {
    { key = "phys:Equal", mods = "NONE", action = act.SendString "´" },
    { key = "phys:Equal", mods = "SHIFT", action = act.SendString "`" },
    { key = "phys:Equal", mods = "OPT", action = act.SendString "`" },
    { key = "phys:RightBracket", mods = "NONE", action = act.SendString "¨" },
    { key = "phys:RightBracket", mods = "SHIFT", action = act.SendString "^" },
    { key = "phys:RightBracket", mods = "OPT", action = act.SendString "~" },
  })
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