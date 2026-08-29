local platform = require "utils.platform"

---@class Modifiers
---@field SUPER string
---@field SUPER_REV string
---@field ALT string
---@field SPECIAL string
---@field SPECIAL_OR_CTRL string[]
---@field SPLITSIDE string

---@return Modifiers
local function modifiers()
  if platform.is_mac then
    ---@type Modifiers
    local m = {
      SUPER = "CMD",
      SUPER_REV = "CMD|SHIFT",
      ALT = "OPT",
      SPECIAL = "CMD",
      SPECIAL_OR_CTRL = { "CMD", "CTRL" },
      SPLITSIDE = "'",
    }

    return m
  end

  ---@type Modifiers
  local m = {
    SUPER = "CTRL",
    SUPER_REV = "CTRL|SHIFT",
    ALT = "ALT",
    SPECIAL = "ALT",
    SPECIAL_OR_CTRL = { "ALT", "CTRL" },
    SPLITSIDE = "§",
  }

  return m
end

local _MOD = modifiers()

return _MOD
