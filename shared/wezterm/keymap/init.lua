local wezterm = require "wezterm" ---@type Wezterm

local M = {}

local mux_route = wezterm.home_dir .. "/.local/bin/mux-route"

local function domain()
  local ran, ok, stdout, stderr = pcall(wezterm.run_child_process, { mux_route })

  if not ran then
    return nil, mux_route .. ": " .. tostring(ok)
  end

  if not ok then
    local reason = (stderr or ""):gsub("%s+$", "")
    return nil, reason ~= "" and reason or "mux-route: no answer"
  end

  return (stdout:gsub("%s+$", ""))
end

M.attach = wezterm.action_callback(function(window, pane)
  local name, reason = domain()
  if not name then
    wezterm.log_error(reason)
    window:toast_notification("wezterm mux", reason, nil, 5000)
    return
  end
  window:perform_action(wezterm.action.AttachDomain(name), pane)
end)

function M.apply_to_config(config)
  local platform = wezterm.target_triple:find "darwin" and "keymap.macos" or "keymap.linux"
  local chord = require(platform)

  config.keys = {
    { key = chord.key, mods = chord.mods, action = M.attach },
  }
end

return M
