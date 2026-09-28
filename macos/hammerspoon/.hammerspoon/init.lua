hs.loadSpoon("EmmyLua")
local keybinds = require "utils.keybinds"

-- Keybinds --

-- fn + n
keybinds.createKeybind("fn", "n", function()
    hs.urlevent.openURL("obsidian://unique?vault=main")
end)
