local wezterm = require "wezterm" ---@type Wezterm

wezterm.GLOBAL.maximized_windows = wezterm.GLOBAL.maximized_windows or {}

-- Fires on window creation and on every config reload, so only maximize once per window.
wezterm.on("window-config-reloaded", function(window, _pane)
  local id = tostring(window:window_id())
  if wezterm.GLOBAL.maximized_windows[id] then
    return
  end

  wezterm.GLOBAL.maximized_windows[id] = true
  window:maximize()
end)
