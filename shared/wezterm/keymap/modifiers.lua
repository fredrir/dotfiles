---@diagnostic disable: missing-fields
local platform = require "utils.platform"

local M = {}

---@type Mods
local MOD

if platform.is_mac then
  MOD = {
    PRIMARY = "CMD",
    SECONDARY = "CTRL",
    EDGE = "CMD",
    ---
    UNIQUE = "OPT",
    SHIFT_RIGHT = "CTRL|ALT|CMD|SHIFT", -- Macos Right Shift Key remapped with Karabiner
    ---
    SUPER_REV = "CMD|SHIFT",
    UNIQUE_REV = "OPT|SHIFT",
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
    UNIQUE_REV = "CTRL|ALT",
    ---
    SPLITBELOW = "§",
  }
end

return MOD ---@ type Mods