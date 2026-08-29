local platform = require "utils.platform"

---@return Modifiers
local function modifiers()
  if platform.is_mac then
    ---@type Modifiers
    return {
      SUPER = "CMD",
      SPECIAL = "CMD",
      ALT = "OPT",
      ---
      SUPER_REV = "CMD|SHIFT",
      SPECIAL_OR_CTRL = { "CMD", "CTRL" },
      ---
      SPLITBELOW = "'",
    }
  end

  ---@type Modifiers
  return {
    SUPER = "CTRL",
    SPECIAL = "ALT",
    ALT = "ALT",
    ---
    SUPER_REV = "CTRL|SHIFT",
    SPECIAL_OR_CTRL = { "ALT", "CTRL" },
    ---
    SPLITBELOW = "§",
  }
end

local _MOD = modifiers()

return _MOD
