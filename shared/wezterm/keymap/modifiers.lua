local platform = require "utils.platform"

---@return Modifiers
local function modifiers()
  if platform.is_mac then
    ---@type Modifiers
    return {
      SUPER = "CMD",
      SPECIAL = "CMD",
      SECONDARY = "OPT",
      ---
      SUPER_REV = "CMD|SHIFT",
      CTRL_OR_SPECIAL = { "CTRL", "CMD" },
      CTRL_OR_SECONDARY = { "CTRL", "OPT" },
      ---
      SPLITBELOW = "'",
    }
  end

  ---@type Modifiers
  return {
    SUPER = "CTRL",
    SPECIAL = "ALT",
    SECONDARY = "ALT",
    ---
    SUPER_REV = "CTRL|SHIFT",
    CTRL_OR_SPECIAL = { "CTRL", "ALT" },
    CTRL_OR_SECONDARY = { "CTRL", "ALT" },

    ---
    SPLITBELOW = "§",
  }
end

local _MOD = modifiers()

return _MOD
