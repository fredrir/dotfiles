hs.loadSpoon("EmmyLua")
hs.loadSpoon("ReloadConfiguration")
local key = require "utils.key"

spoon.ReloadConfiguration:start()

-- Keybinds --

-- Reload
key.createKeybind("fn", "r", function()
    hs.reload()
end)

-- Obsidian --
key.createKeybind("fn", "n", function()
    hs.urlevent.openURL("obsidian://unique?vault=main")
end)

-- Zen Browser --
key.openApp("fn", "b", "Zen")

-- Wezterm --
key.openApp("fn", ".", "Wezterm")
key.openApp("fn", "t", "Wezterm")

-- Obsidian --
key.openApp("fn", "o", "Obsidian")

-- Vscode --
key.openApp("fn", "p", "Visual Studio Code")
