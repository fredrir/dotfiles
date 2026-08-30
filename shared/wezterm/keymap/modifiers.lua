local platform = require "utils.platform"

---@return Mods
local function modifiers()
  if platform.is_mac then
    ---@type Mods
    return {
      SUPER = "CMD",
      SPECIAL = "CMD",
      SECONDARY = "OPT",
      ---
      SUPER_REV = "CMD|SHIFT",
      CTRL_OR_SPECIAL = { "CTRL", "CMD" },
      CTRL_OR_SECONDARY = { "CTRL", "OPT" },
      CTRL_OR_SECONDARY_OR_SPECIAL = { "CTRL", "OPT", "CMD" },
      ---
      SPLITBELOW = "'",
    }
  end

  ---@type Mods
  return {
    SUPER = "CTRL",
    SPECIAL = "ALT",
    SECONDARY = "ALT",
    ---
    SUPER_REV = "CTRL|SHIFT",
    CTRL_OR_SPECIAL = { "CTRL", "ALT" },
    CTRL_OR_SECONDARY = { "CTRL", "ALT" },
    CTRL_OR_SECONDARY_OR_SPECIAL = { "CTRL", "OPT", "CMD" },

    ---
    SPLITBELOW = "§",
  }
end

local _MOD = modifiers()

return _MOD
