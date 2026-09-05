local wezterm = require "wezterm"
local act = wezterm.action

local M = {}
local overrides = {}
local last_routes = {}

-- Set by the tmux client attach/detach lifecycle on its actual terminal. Shell
-- environment variables describe ancestry and cannot establish GUI ownership.
function M.active(pane)
  local override = overrides[pane:pane_id()]
  if override ~= nil then
    return override
  end
  return pane:get_user_vars().TMUX_WORKSPACE == "1"
end

function M.dispatch(key, fallback)
  return wezterm.action_callback(function(window, pane)
    if M.active(pane) then
      last_routes[pane:pane_id()] = "WezTerm → tmux · Ctrl-b " .. key
      window:perform_action(
        act.Multiple {
          act.SendKey { key = "b", mods = "CTRL" },
          act.SendKey { key = key, mods = "NONE" },
        },
        pane
      )
    else
      last_routes[pane:pane_id()] = "WezTerm action"
      window:perform_action(fallback, pane)
    end
  end)
end

-- Useful with an older remote configuration. The next toggle returns to auto;
-- a real lifecycle event always clears a manual override.
M.toggle = wezterm.action_callback(function(window, pane)
  local id = pane:pane_id()
  local mode
  if overrides[id] ~= nil then
    overrides[id] = nil
    mode = "automatic"
  else
    overrides[id] = not M.active(pane)
    mode = "manual"
  end
  window:toast_notification("Key routing", (M.active(pane) and "tmux" or "WezTerm") .. " · " .. mode, nil, 3500)
end)

M.inspect = wezterm.action_callback(function(window, pane)
  local owner = M.active(pane) and "tmux" or "WezTerm"
  local marker = pane:get_user_vars().TMUX_WORKSPACE or "unset"
  local source = overrides[pane:pane_id()] == nil and "automatic" or "manual override"
  window:toast_notification(
    "Key routing · " .. owner,
    source
      .. " · TMUX_WORKSPACE="
      .. marker
      .. " · pane "
      .. pane:pane_id()
      .. "\nLast routed shortcut: "
      .. (last_routes[pane:pane_id()] or "none")
      .. "\nPrimary+Shift+F12 toggles routing; tmux prefix Ctrl-b remains portable.",
    nil,
    7000
  )
  if M.active(pane) then
    window:perform_action(
      act.Multiple {
        act.SendKey { key = "b", mods = "CTRL" },
        act.SendKey { key = "I", mods = "NONE" },
      },
      pane
    )
  end
end)

wezterm.on("user-var-changed", function(_, pane, name)
  if name == "TMUX_WORKSPACE" then
    overrides[pane:pane_id()] = nil
  end
end)

return M
