local wezterm = require "wezterm" ---@type Wezterm
local hwire_session = require "utils.hwire-session"

local act = wezterm.action

local MUX_ROUTE = wezterm.home_dir .. "/.local/bin/mux-route"
local TOAST_MS = 4000

---@param window Window
---@param pane Pane
local function attach_peer(window, pane)
  local ok, stdout, stderr = wezterm.run_child_process { MUX_ROUTE }
  if not ok then
    local reason = stderr:gsub("%s+$", "")
    window:toast_notification(
      "wezterm",
      "mux-route failed: " .. (reason ~= "" and reason or "no route answered"),
      nil,
      TOAST_MS
    )
    return
  end

  local domain = stdout:gsub("%s+$", "")
  if domain == "" then
    window:toast_notification("wezterm", "mux-route returned no domain", nil, TOAST_MS)
    return
  end

  local command = { domain = { DomainName = domain } }
  local session = hwire_session.for_domain(domain)
  if session then
    command.args = { "zsh", "-l" }
    command.set_environment_variables = { HWIRE_SESSION = session }
  end

  window:perform_action(act.SpawnCommandInNewTab(command), pane)
end

return wezterm.action_callback(attach_peer)
