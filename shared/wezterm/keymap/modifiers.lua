---@diagnostic disable: missing-fields
local platform = require "utils.platform"

---@type Mods
local MOD

if platform.is_mac then
  MOD = {
    PRIMARY = "CMD",
    SECONDARY = "CTRL",
    EDGE = "CMD",
    ---
    UNIQUE = "OPT",
    SHIFT_RIGHT = "CTRL|ALT|CMD", -- Macos Right Shift Key remapped with Karabiner
    ---
    SUPER_REV = "CMD|SHIFT",
    UNIQUE_REV = "OPT|SHIFT",
    SYSTEM_REV = "CMD|OPT",
    SHIFT_RIGHT_REV = "CTRL|ALT|CMD|SHIFT",
    ---
    SPLITBELOW = "'",
  }
else
  MOD = {
    PRIMARY = "CTRL",
    SECONDARY = "ALT",
    EDGE = "ALT",
    ---
    UNIQUE = "ALT",
    SHIFT_RIGHT = "CTRL|ALT|SHIFT",
    ---
    SUPER_REV = "CTRL|SHIFT",
    UNIQUE_REV = "ALT|SHIFT",
    SYSTEM_REV = "CTRL|ALT",
    ---
    SPLITBELOW = "§",
  }
end

-- Mods for a binding's peer host action
MOD.REV = platform.is_mac and {
  [MOD.UNIQUE] = MOD.UNIQUE_REV,
  [MOD.SHIFT_RIGHT] = MOD.SHIFT_RIGHT_REV,
} or {}

return MOD ---@ type Mods