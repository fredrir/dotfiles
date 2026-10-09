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
    UNIQUE_LEFT = "OPT", -- Normal Macos Option Key
    UNIQUE_RIGHT = "CTRL|ALT|CMD|SHIFT", -- Macos Right Option Key remapped with Karabiner
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
    UNIQUE_LEFT = "ALT",
    UNIQUE_RIGHT = "ALT",
    ---
    SUPER_REV = "CTRL|SHIFT",
    UNIQUE_REV = "CTRL|ALT",
    ---
    SPLITBELOW = "§",
  }
end

MOD.UNIQUE_XOR = { MOD.UNIQUE_LEFT, MOD.UNIQUE_RIGHT }

return MOD ---@ type Mods