local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local MOD = require "keymap.modifiers"
local motion_keys = require "keymap.motion-keys"
local act = wezterm.action

local shortcut_mods = platform.is_mac and MOD.PRIMARY or "CTRL|SHIFT"

---@alias FreeCursorHandler fun(window: Window, pane: Pane)
---@type table<string, FreeCursorHandler>
local handlers = {}

---@param name string
---@param handler FreeCursorHandler
---@return Action
local function callback(name, handler)
  local event = "dotfiles-free-cursor:" .. name
  if not handlers[event] then
    wezterm.on(event, function(window, pane)
      handlers[event](window, pane)
    end)
  end
  handlers[event] = handler
  return act.EmitEvent(event)
end

---@type table<integer, boolean>
local selecting = {}

---@param window Window
---@param pane Pane
local function leave(window, pane)
  selecting[pane:pane_id()] = nil
  window:perform_action(
    act.Multiple {
      act.CopyMode "ClearSelectionMode",
      act.CopyMode "Close",
      act.ScrollToBottom,
    },
    pane
  )
end

---@param name string
---@param action Action
---@return Action
local function resume(name, action)
  return callback("resume:" .. name, function(window, pane)
    leave(window, pane)
    window:perform_action(action, pane)
  end)
end

---@param name string
---@param movement CopyModeParams
---@param select boolean
---@return Action
local function move(name, movement, select)
  return callback("move:" .. name, function(window, pane)
    local id = pane:pane_id()
    ---@type Action[]
    local actions = {}
    if select then
      if not selecting[id] then
        selecting[id] = true
        table.insert(actions, act.CopyMode { SetSelectionMode = "Cell" })
      end
    else
      selecting[id] = nil
      table.insert(actions, act.CopyMode "ClearSelectionMode")
    end
    table.insert(actions, act.CopyMode(movement))
    window:perform_action(act.Multiple(actions), pane)
  end)
end

local activate = callback("activate", function(window, pane)
  selecting[pane:pane_id()] = nil
  window:perform_action(act.ActivateCopyMode, pane)
  window:perform_action(act.CopyMode "ClearSelectionMode", pane)
  window:perform_action(act.CopyMode "ClearPattern", pane)
end)

---@param bindings Key[]
---@return Key[]
local function key_table(bindings)
  ---@type Key[]
  local keys = {}
  ---@type table<string, integer>
  local positions = {}

  ---@param key string
  ---@param mods string
  ---@param action Action
  local function bind(key, mods, action)
    local id = key .. ":" .. mods
    local index = positions[id] or (#keys + 1)
    positions[id] = index
    keys[index] = { key = key, mods = mods, action = action }
  end

  for codepoint = 0x20, 0x24f do
    if codepoint < 0x7f or codepoint >= 0xa0 then
      local char = utf8.char(codepoint)
      local action = resume(("text:%x"):format(codepoint), act.SendString(char))
      bind("mapped:" .. char, "NONE", action)
      if not char:match "^[A-Za-z]$" then
        bind("mapped:" .. char, "SHIFT", action)
      end
    end
  end

  for codepoint = string.byte "a", string.byte "z" do
    local key = string.char(codepoint)
    bind(key, "CTRL", resume("key:" .. key .. ":CTRL", act.SendKey { key = key, mods = "CTRL" }))
  end

  for _, key in ipairs { "Enter", "Tab", "Backspace", "Delete" } do
    for _, mods in ipairs { "NONE", "SHIFT", "CTRL", "ALT" } do
      bind(key, mods, resume("key:" .. key .. ":" .. mods, act.SendKey { key = key, mods = mods }))
    end
  end

  for _, binding in ipairs(bindings) do
    bind(binding.key, binding.mods, resume("binding:" .. binding.key .. ":" .. binding.mods, binding.action))
  end

  ---@type { key: string, movement: CopyModeParams }[]
  local movements = {
    { key = "LeftArrow", movement = "MoveLeft" },
    { key = "RightArrow", movement = "MoveRight" },
    { key = "UpArrow", movement = "MoveUp" },
    { key = "DownArrow", movement = "MoveDown" },
    { key = "Home", movement = "MoveToStartOfLine" },
    { key = "End", movement = "MoveToEndOfLineContent" },
    { key = "PageUp", movement = "PageUp" },
    { key = "PageDown", movement = "PageDown" },
  }
  for _, motion in ipairs(movements) do
    bind(motion.key, "NONE", move(motion.key .. ":NONE", motion.movement, false))
    bind(motion.key, "SHIFT", move(motion.key .. ":SHIFT", motion.movement, true))
  end

  for _, binding in ipairs(motion_keys) do
    if binding.free_cursor then
      bind(
        binding.key,
        binding.mods,
        move(binding.key .. ":" .. binding.mods, binding.free_cursor, binding.mods:find("SHIFT", 1, true) ~= nil)
      )
    end
  end

  bind("c", shortcut_mods, act.CopyTo "Clipboard")
  local exit = callback("leave", leave)
  bind("e", shortcut_mods, exit)
  bind("Escape", "NONE", exit)

  return keys
end

return { activate = activate, key_table = key_table }