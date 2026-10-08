require "hs.ipc"
hs.loadSpoon "EmmyLua"
hs.loadSpoon "ReloadConfiguration"
local key = require "utils.key"

spoon.ReloadConfiguration:start()

-- Keybinds --

-- Reload
key.createKeybind({ "fn" }, "r", function()
  hs.reload()
end)

-- Obsidian --
key.createKeybind({ "fn" }, "n", function()
  hs.urlevent.openURL "obsidian://unique?vault=main"
end)

-- Zen Browser --
key.openApp({ "fn" }, "b", "Zen")

-- Wezterm --
key.openApp({ "fn" }, ".", "Wezterm")
key.openApp({ "fn" }, "t", "Wezterm")

-- Obsidian --
key.openApp({ "fn" }, "o", "Obsidian")

-- Vscode --
key.openApp({ "fn" }, "p", "Visual Studio Code")

-- Window tiling --
-- fn+arrows arrive as home/end/pageup/pagedown
local tiles = {
  home = { menu = { "Window", "Move & Resize", "Left" }, unit = hs.layout.left50 },
  ["end"] = { menu = { "Window", "Move & Resize", "Right" }, unit = hs.layout.right50 },
  pageup = { menu = { "Window", "Move & Resize", "Top" }, unit = { x = 0, y = 0, w = 1, h = 0.5 } },
  pagedown = { menu = { "Window", "Move & Resize", "Bottom" }, unit = { x = 0, y = 0.5, w = 1, h = 0.5 } },
  f = { menu = { "Window", "Fill" }, unit = hs.layout.maximized },
}

for k, tile in pairs(tiles) do
  key.createKeybind({ "fn", "ctrl" }, k, function()
    local win = hs.window.focusedWindow()
    if not win then
      return
    end
    ---@type hs.application|nil
    local app = win:application()
    if not app or not app:selectMenuItem(tile.menu) then
      win:moveToUnit(tile.unit, 0)
    end
  end)
end

-- Move window between monitors --
local neighbours = {
  pageup = function(screen)
    return screen:toNorth()
  end,
  pagedown = function(screen)
    return screen:toSouth()
  end,
}

for k, neighbour in pairs(neighbours) do
  key.createKeybind({ "fn", "alt" }, k, function()
    local win = hs.window.focusedWindow()
    local target = win and neighbour(win:screen())
    if target then
      local frame = target:frame()
      win:setFrame(frame)
      hs.mouse.absolutePosition(frame.center)
      win:focus()
    end
  end)
end

-- Mouse between monitors --
local lastMouse = {}

key.createKeybind({ "fn" }, "m", function()
  ---@type hs.screen|nil
  local from = hs.mouse.getCurrentScreen()
  if not from then
    return
  end
  local to = from:next()
  lastMouse[from:getUUID()] = hs.mouse.absolutePosition()
  hs.mouse.absolutePosition(lastMouse[to:getUUID()] or to:frame().center)

  local position = hs.geometry.new(hs.mouse.absolutePosition())
  local windows = hs.window.orderedWindows()
  local sourceWindow
  for _, win in ipairs(windows) do
    if win:screen():id() == from:id() then
      sourceWindow = win
      break
    end
  end

  for _, win in ipairs(windows) do
    if position:inside(win:frame()) then
      win:focus()
      -- App activation can also bring a sibling window forward on the source screen.
      if sourceWindow and sourceWindow:id() ~= win:id() then
        sourceWindow:raise()
      end
      win:raise():becomeMain()
      break
    end
  end
end)
