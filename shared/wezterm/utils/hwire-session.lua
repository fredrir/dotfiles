local wezterm = require "wezterm" ---@type Wezterm
local host = require "domain.hosts"

local act = wezterm.action

local hwire_session = {}

---@param domain string
---@return string?
function hwire_session.for_domain(domain)
  for _, route in ipairs(host.target.ip) do
    if domain == ("%s-%s"):format(host.target.hostname, route.name) then
      return ("v1:%s:%s:%s:tls"):format(host.origin.hostname, host.target.hostname, route.name)
    end
  end

  return nil
end

---@param pane Pane
---@return SpawnCommand
local function command_for_pane(pane)
  local command = { domain = "CurrentPaneDomain" }
  local session = hwire_session.for_domain(pane:get_domain_name())

  if session then
    command.args = { "zsh", "-l" }
    command.set_environment_variables = { HWIRE_SESSION = session }
  end

  return command
end

hwire_session.new_tab = wezterm.action_callback(function(window, pane)
  window:perform_action(act.SpawnCommandInNewTab(command_for_pane(pane)), pane)
end)

---@param direction "horizontal" | "vertical"
---@return Action
function hwire_session.split(direction)
  local vtabs = require("plugins.vtabs").plugin
  return vtabs.action.split(direction == "horizontal" and "Right" or "Bottom", command_for_pane)
end

return hwire_session
