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
    SUPER_REV = "CMD|SHIFT",
    SUPER_REV_2 = "CTRL|ALT|CMD|SHIFT",
    ---
    UNIQUE_LEFT = "OPT", -- Normal Macos Option Key
    UNIQUE_RIGHT = "CTRL|ALT|CMD|SHIFT", -- Macos Right Option Key remapped with Karabiner
    UNIQUE_REV = "OPT|SHIFT",
    ---
    SPLITBELOW = "'",
  }
else
  MOD = {
    PRIMARY = "CTRL",
    SECONDARY = "ALT",
    UNIQUE_LEFT = "ALT",
    UNIQUE_RIGHT = "ALT",
    EDGE = "ALT",

    SUPER_REV = "CTRL|SHIFT",
    SUPER_REV_2 = "CTRL|ALT",
    UNIQUE_REV = "CTRL|ALT",
    SPLITBELOW = "§",
  }
end

MOD.UNIQUE_XOR = { MOD.UNIQUE_LEFT, MOD.UNIQUE_RIGHT }

return MOD ---@ type Mods