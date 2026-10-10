local wezterm = require "wezterm" ---@type Wezterm

wezterm.on("window-config-reloaded", function(window, _pane)
  local id = tostring(window:window_id())

  local maximized_windows = wezterm.GLOBAL.maximized_windows
  if type(maximized_windows) ~= "table" then
    maximized_windows = {}
  end

  ---@cast maximized_windows table<string, boolean>

  if maximized_windows[id] then
    return
  end

  maximized_windows[id] = true
  wezterm.GLOBAL.maximized_windows = maximized_windows

  window:maximize()
end)