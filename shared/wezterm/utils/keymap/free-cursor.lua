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

---@alias FreeCursorCell { x: integer, y: integer }
---@type table<integer, FreeCursorCell>
local jumps = {}

---@param window Window
---@param pane Pane
---@param preserve_selection? boolean
local function leave(window, pane, preserve_selection)
  selecting[pane:pane_id()] = nil
  jumps[pane:pane_id()] = nil
  ---@type Action[]
  local actions = { act.CopyMode "Close", act.ScrollToBottom }
  if not preserve_selection then
    table.insert(actions, 1, act.CopyMode "ClearSelectionMode")
  end
  window:perform_action(act.Multiple(actions), pane)
end

---@param name string
---@param action Action
---@param preserve_selection? boolean
---@return Action
local function resume(name, action, preserve_selection)
  return callback("resume:" .. name, function(window, pane)
    leave(window, pane, preserve_selection)
    window:perform_action(action, pane)
  end)
end

---@param pane Pane
---@param select boolean
---@return Action[]
local function selection_actions(pane, select)
  local id = pane:pane_id()
  if not select then
    selecting[id] = nil
    return { act.CopyMode "ClearSelectionMode" }
  end
  if selecting[id] then
    return {}
  end
  selecting[id] = true
  return { act.CopyMode { SetSelectionMode = "Cell" } }
end

---@param name string
---@param movement CopyModeParams
---@param select boolean
---@return Action
local function move(name, movement, select)
  return callback("move:" .. name, function(window, pane)
    local actions = selection_actions(pane, select)
    table.insert(actions, act.CopyMode(movement))
    window:perform_action(act.Multiple(actions), pane)
  end)
end

---@param pane Pane
---@param cols integer
---@param row integer
---@return integer?
local function command_column(pane, cols, row)
  local text = pane:get_text_from_region(0, row, cols, row)
  local prompt = text:match "^%[.-%]%$" or text:match "^%(%.venv%)%[.-%]%$"
  return prompt and wezterm.column_width(prompt) + 1
end

---@param pane Pane
---@param cols integer
---@param from integer
---@param to integer
---@param step -1|1
---@return FreeCursorCell?
local function find_command(pane, cols, from, to, step)
  for row = from, to, step do
    local x = command_column(pane, cols, row)
    if x then
      return { x = x, y = row }
    end
  end
end

-- Last row that can hold a finished command, above the live prompt when one is shown.
---@param pane Pane
---@param dims RenderableDimensions
---@param cursor StableCursorPosition
---@return integer
local function last_command_row(pane, dims, cursor)
  local editing = pane:get_user_vars().ZLE_EDIT == "1"
  local top = editing and math.max(cursor.y - dims.viewport_rows, dims.scrollback_top) or cursor.y
  local live = find_command(pane, dims.cols, cursor.y, top, -1)
  return live and live.y - 1 or cursor.y
end

-- Lua cannot read the copy mode cursor, so travel starts from the scrollback bottom.
---@param actions Action[]
---@param dims RenderableDimensions
---@param target FreeCursorCell
local function travel(actions, dims, target)
  local rows = dims.scrollback_top + dims.scrollback_rows - 1 - target.y
  table.insert(actions, act.CopyMode "MoveToScrollbackBottom")
  table.insert(actions, act.CopyMode "MoveToStartOfLine")
  for _ = 1, rows // dims.viewport_rows do
    table.insert(actions, act.CopyMode "PageUp")
  end
  for _ = 1, rows % dims.viewport_rows do
    table.insert(actions, act.CopyMode "MoveUp")
  end
  for _ = 1, target.x do
    table.insert(actions, act.CopyMode "MoveRight")
  end
end

---@param name string
---@param step -1|1
---@param select boolean
---@return Action
local function jump(name, step, select)
  return callback("jump:" .. name, function(window, pane)
    local id = pane:pane_id()
    local dims = pane:get_dimensions()
    local cursor = pane:get_cursor_position()
    local last = last_command_row(pane, dims, cursor)
    local origin = jumps[id]
    if step < 0 then
      local target = find_command(pane, dims.cols, origin and origin.y - 1 or last, dims.scrollback_top, -1)
      if not target then
        return
      end
      jumps[id] = target
    elseif origin then
      jumps[id] = find_command(pane, dims.cols, origin.y + 1, last, 1)
    else
      return
    end
    local actions = selection_actions(pane, select)
    travel(actions, dims, jumps[id] or { x = cursor.x, y = cursor.y })
    window:perform_action(act.Multiple(actions), pane)
  end)
end

local activate = callback("activate", function(window, pane)
  selecting[pane:pane_id()] = nil
  jumps[pane:pane_id()] = nil
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
    -- The Backspace callback needs the selection after copy mode closes.
    local preserve_selection = binding.key == "Backspace" and binding.mods == "NONE"
    bind(
      binding.key,
      binding.mods,
      resume("binding:" .. binding.key .. ":" .. binding.mods, binding.action, preserve_selection)
    )
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

  bind("UpArrow", MOD.UNIQUE, jump("previous", -1, false))
  bind("UpArrow", MOD.UNIQUE .. "|SHIFT", jump("previous:select", -1, true))
  bind("DownArrow", MOD.UNIQUE, jump("next", 1, false))
  bind("DownArrow", MOD.UNIQUE .. "|SHIFT", jump("next:select", 1, true))

  bind("c", shortcut_mods, act.CopyTo "Clipboard")
  local exit = callback("leave", leave)
  bind("e", shortcut_mods, exit)
  bind("Escape", "NONE", exit)

  return keys
end

return { activate = activate, key_table = key_table }