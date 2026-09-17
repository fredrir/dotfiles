local wezterm = require "wezterm" ---@type Wezterm
local dotfile = require "utils.dotfile"
local hwire_session = require "utils.hwire-session"
local host = require "domain.hosts"
local ssh_hosts = require "domain.ssh-hosts"
local ssh_mux = require "domain.ssh-mux"

local MUX_ROUTE = dotfile.compiled_dir .. "/mux-route"
local SOCKET = wezterm.home_dir .. "/.local/share/wezterm/localmux.sock"
local CLI = wezterm.executable_dir .. "/wezterm"
local pending = {}

local function fail(_, message)
  wezterm.log_error(message)
end

local function trim(text)
  return (text:gsub("%s+$", ""))
end

local function mux(...)
  local args = { "/usr/bin/env", "WEZTERM_UNIX_SOCKET=" .. SOCKET, CLI, "cli", "--prefer-mux", "--no-auto-start" }
  for _, arg in ipairs { ... } do
    table.insert(args, tostring(arg))
  end
  return wezterm.run_child_process(args)
end

local ISOLATED = '/usr/bin/env -i WEZTERM_PANE="$WEZTERM_PANE" WEZTERM_UNIX_SOCKET="$WEZTERM_UNIX_SOCKET" "$@"'
local LOGIN_SHELL = '"${SHELL:-/bin/sh}" -l'

-- Once the session shell exits, ask the GUI to swap the pane back to a local shell
local RETURN_TO_ORIGIN = [[
request=$(printf 'v1:%s:%s:%s.0' "$ATTACH_MUX_ORIGIN" "$$" "$(date +%s)" | base64 | tr -d '\n')
printf '\033]1337;SetUserVar=ATTACH_MUX=%s\007' "$request"
sleep 5]]

---@param domain string
---@param cwd string?
---@param script string
---@param ... string
---@return string[]
local function spawn(domain, cwd, script, ...)
  local args = { "--domain-name", domain }
  if cwd then
    table.insert(args, "--cwd")
    table.insert(args, cwd)
  end
  local command =
    { "--", "/usr/bin/env", "ATTACH_MUX_ORIGIN=" .. host.origin.hostname, "/bin/sh", "-c", script, "attach_mux", ... }
  for _, arg in ipairs(command) do
    table.insert(args, arg)
  end
  return args
end

---@param shell string
---@return string
local function returning(shell)
  return shell .. "\n" .. RETURN_TO_ORIGIN
end

---@param home string
---@param session string
---@return string ...
local function isolated_zsh(home, session)
  return "HOME=" .. home,
    "TERM=xterm-256color",
    "PATH=/usr/local/bin:/usr/bin:/bin",
    "HWIRE_SESSION=" .. session,
    "zsh",
    "-l"
end

---@return string[]?
local function split_args(window, target)
  if target == host.origin.hostname then
    return spawn("local", host.origin.home, "exec " .. ISOLATED, isolated_zsh(host.origin.home, ""))
  end

  if target == host.target.hostname then
    local ok, stdout, stderr = wezterm.run_child_process { MUX_ROUTE, target }
    if not ok then
      fail(window, "mux-route: " .. trim(stderr))
      return
    end
    local domain = trim(stdout)
    local session = hwire_session.for_domain(domain)
    if not session then
      fail(window, "mux-route: invalid domain")
      return
    end
    return spawn(domain, host.target.home, returning(ISOLATED), isolated_zsh(host.target.home, session))
  end

  local remote = ssh_mux.hosts[target]
  if remote then
    if not ssh_mux.supported then
      fail(window, "attach_mux: " .. target .. " needs a WezTerm build with unix local_pane_layout")
      return
    end
    return spawn(target, remote.home, returning(ISOLATED), isolated_zsh(remote.home, ""))
  end

  for _, name in ipairs(ssh_hosts()) do
    if name == target then
      return spawn(target, nil, returning(LOGIN_SHELL))
    end
  end

  fail(window, "attach_mux: unknown host: " .. target)
end

local function replace(window, pane, target)
  local metadata = pane:get_metadata() or {}
  local source = metadata.remote_pane_id
  if pane:get_domain_name() ~= "localmux" or type(source) ~= "number" then
    fail(window, "attach_mux requires localmux and the updated WezTerm build")
    return
  end

  local args = split_args(window, target)
  if not args then
    return
  end

  local ok, stdout, stderr = mux("split-pane", "--pane-id", source, table.unpack(args))
  if not ok then
    fail(window, "attach_mux: " .. trim(stderr))
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
    fail(window, "attach_mux: " .. trim(close_error))
    return
  end
  mux("activate-pane", "--pane-id", replacement)
end

local function attach(window, pane, target)
  target = target == "peer" and host.target.hostname or target
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
  local target = value:match "^v1:([%w._-]+):%d+:%d+%.%d+$"
  if target then
    attach(window, pane, target)
  end
end)
