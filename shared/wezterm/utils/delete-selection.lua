local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action

---@type table<integer, InputSelection>
local pending = {}

wezterm.on("user-var-changed", function(window, pane, name, value)
  local id = pane:pane_id()
  if name == "ZLE_EDIT" and value ~= "1" then
    pending[id] = nil
  end
  if name ~= "ZLE_SELECTION_REQUEST" then
    return
  end
  local selected = pending[id]
  pending[id] = nil
  if not selected then
    return
  end
  local serial, cursor, buffer = value:match "^(%d+):(%d+):(.*)$"
  local cursor_offset = tonumber(cursor)
  if not serial or not cursor_offset or cursor_offset > #buffer or pane:get_user_vars().ZLE_EDIT ~= "1" then
    return
  end
  ---@type {start: integer, ['end']: integer}?
  local range = selected:get_range(buffer, math.floor(cursor_offset))
  local reply = range and (range.start .. ":" .. range["end"]) or "-1:-1"
  window:perform_action(act.SendString("\x1b[5;30023~" .. serial .. ":" .. reply .. "\a"), pane)
  if range then
    window:perform_action(act.ClearSelection, pane)
  end
end)

return wezterm.action_callback(function(window, pane)
  if pane:get_user_vars().ZLE_EDIT ~= "1" or pane:is_alt_screen_active() then
    window:perform_action(act.SendKey { key = "Backspace", mods = "NONE" }, pane)
    return
  end
  local id = pane:pane_id()
  if pending[id] then
    return
  end
  local ok, snapshot = pcall(function()
    return window:get_input_selection(pane)
  end)
  if not ok then
    if window:get_selection_text_for_pane(pane) == "" then
      window:perform_action(act.SendKey { key = "Backspace", mods = "NONE" }, pane)
    else
      window:toast_notification("WezTerm", "Restart WezTerm with the selection editing build", nil, 3000)
    end
    return
  end
  if not snapshot then
    if window:get_selection_text_for_pane(pane) == "" then
      window:perform_action(act.SendKey { key = "Backspace", mods = "NONE" }, pane)
    end
    return
  end
  pending[id] = snapshot
  wezterm.time.call_after(2, function()
    if pending[id] == snapshot then
      pending[id] = nil
    end
  end)
  window:perform_action(act.SendString "\x1b[5;30022~", pane)
end)