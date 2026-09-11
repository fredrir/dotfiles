local wezterm = require "wezterm" ---@type Wezterm
local hwire_session = require "utils.hwire-session"
local host = require "domain.hosts"

local MUX_ROUTE = wezterm.home_dir .. "/.local/bin/mux-route"
local SOCKET = wezterm.home_dir .. "/.local/share/wezterm/localmux.sock"
local TOAST_MS = 4000
local pending = {}

local function fail(window, message)
  window:toast_notification("wezterm", message, nil, TOAST_MS)
end

local function mux(...)
  local args = { "env", "WEZTERM_UNIX_SOCKET=" .. SOCKET, "wezterm", "cli", "--prefer-mux", "--no-auto-start" }
  for _, arg in ipairs { ... } do
    table.insert(args, tostring(arg))
  end
  return wezterm.run_child_process(args)
end

local function replace(window, pane, target)
  local metadata = pane:get_metadata() or {}
  local source = metadata.remote_pane_id
  if pane:get_domain_name() ~= "localmux" or type(source) ~= "number" then
    fail(window, "attach_mux requires localmux and the updated WezTerm build")
    return
  end

  local domain, home, session
  if target == host.origin.hostname then
    domain, home = "local", host.origin.home
  else
    local ok, stdout, stderr = wezterm.run_child_process { MUX_ROUTE, host.target.hostname }
    if not ok then
      fail(window, "mux-route: " .. stderr:gsub("%s+$", ""))
      return
    end
    domain = stdout:gsub("%s+$", "")
    session = hwire_session.for_domain(domain)
    if not session then
      fail(window, "mux-route: invalid domain")
      return
    end
    home = host.target.home
  end

  local args = { "env", "HOME=" .. home, "TERM=xterm-256color", "PATH=/usr/local/bin:/usr/bin:/bin" }
  table.insert(args, "HWIRE_SESSION=" .. (session or ""))
  table.insert(args, "zsh")
  table.insert(args, "-l")

  local command = { "split-pane", "--pane-id", source, "--domain-name", domain, "--cwd", home, "--" }
  for _, arg in ipairs(args) do
    table.insert(command, arg)
  end
  local ok, stdout, stderr = mux(table.unpack(command))
  if not ok then
    fail(window, "attach_mux: " .. stderr:gsub("%s+$", ""))
    return
  end
  local replacement = tonumber(stdout:match "^%s*(%d+)%s*$")
  if not replacement or replacement == source then
    fail(window, "attach_mux: invalid replacement pane")
    return
  end
  local closed, _, close_error = mux("kill-pane", "--pane-id", source)
  if not closed then
    mux("kill-pane", "--pane-id", replacement)
    fail(window, "attach_mux: " .. close_error:gsub("%s+$", ""))
    return
  end
  mux("activate-pane", "--pane-id", replacement)
end

local function attach(window, pane, target)
  target = target == "peer" and host.target.hostname or target
  if target ~= host.origin.hostname and target ~= host.target.hostname then
    return
  end
  local id = pane:pane_id()
  if pending[id] then
    return
  end
  pending[id] = true
  local ok, reason = pcall(replace, window, pane, target)
  pending[id] = nil
  if not ok then
    fail(window, "attach_mux: " .. tostring(reason))
  end
end

wezterm.on("user-var-changed", function(window, pane, name, value)
  if name ~= "ATTACH_MUX" then
    return
  end
  local target = value:match "^v1:([a-z]+):%d+:%d+%.%d+$"
  if target then
    attach(window, pane, target)
  end
end)

return wezterm.action_callback(function(window, pane)
  attach(window, pane, "peer")
end)
