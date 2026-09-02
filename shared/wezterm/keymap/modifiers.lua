local platform = require "utils.platform"

---@type Mods
local MOD

if platform.is_mac then
  MOD = {
    PRIMARY = "CMD",
    SECONDARY = "CTRL",
    UNIQUE = "OPT",
    EDGE = "CMD",
    ---
    SUPER_REV = "CMD|SHIFT",
    SUPER_REV_2 = "CTRL|CMD",
    ---
    SPLITBELOW = "'",
  }
else
  MOD = {
    PRIMARY = "CTRL",
    SECONDARY = "ALT",
    UNIQUE = "ALT",
    EDGE = "ALT",

    SUPER_REV = "CTRL|SHIFT",
    SUPER_REV_2 = "CTRL|ALT",

    SPLITBELOW = "§",
  }
end

-- EDGE; What should be CMD on mac, but is not naturally reserved for alt-key in linux. CTRL key with mac-keyboard is less ergonomical and more akward than CTRL on windows due the placement of the "fn" button being bottom left.
-- UNQIUE: The key that is unique between macos and linux; macos has one more unique usable key due to WM in linux owning SUPER.

return MOD ---@ type Mods
