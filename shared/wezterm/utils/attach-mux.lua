local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action
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

---@return table?, string?
local function mux_json(...)
  local ok, stdout, stderr = mux(...)
  if not ok then
    return nil, trim(stderr)
  end
  return wezterm.json_parse(stdout)
end

---@param target string
---@return string?
local function peer_domain(window, target)
  local ok, stdout, stderr = wezterm.run_child_process { MUX_ROUTE, target }
  if not ok then
    fail(window, "mux-route: " .. trim(stderr))
    return
  end
  return trim(stdout)
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
    "COLORTERM=truecolor",
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
    local domain = peer_domain(window, target)
    if not domain then
      return
    end
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

---@return integer? localmux pane ID behind the GUI pane
local function localmux_pane(window, pane)
  local metadata = pane:get_metadata() or {}
  local source = metadata.remote_pane_id
  if pane:get_domain_name() ~= "localmux" or type(source) ~= "number" then
    fail(window, "attach_mux requires localmux and the updated WezTerm build")
    return
  end
  return source
end

local function replace(window, pane, target)
  local source = localmux_pane(window, pane)
  if not source then
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

---@return string?
local function adoption_domain(window, target)
  if target == host.target.hostname then
    return peer_domain(window, target)
  end
  if ssh_mux.hosts[target] and ssh_mux.supported then
    return target
  end
  fail(window, "attach_mux: " .. target .. " has no mux to adopt from")
end

local function describe(entry, place)
  local cwd = (entry.cwd or ""):gsub("^file://[^/]*", "")
  return ("%s  %s  (%s)"):format(entry.title, cwd, place)
end

local function show(window, choice, domain, window_id)
  local kind, pane_id = choice:match "^(%a+):(%d+)$"
  local detached = kind == "detached"
  local args = detached and { "move-pane-to-new-tab", "--pane-id", pane_id, "--window-id", window_id }
    or { "adopt-pane", "--domain-name", domain, "--remote-pane-id", pane_id, "--window-id", window_id }
  local ok, stdout, stderr = mux(table.unpack(args))
  if not ok then
    fail(window, "attach_mux: " .. trim(stderr))
    return
  end
  mux("activate-pane", "--pane-id", detached and pane_id or trim(stdout))
end

-- Offers detached tabs and the target's unowned panes as new tabs in this window
local function adopt(window, pane, target)
  local source = localmux_pane(window, pane)
  local domain = source and adoption_domain(window, target)
  if not domain then
    return
  end
  -- Listing attaches the domain, which first restores orphaned shells as detached tabs
  local remote, list_error = mux_json("adopt-pane", "--domain-name", domain, "--list", "--format", "json")
  local panes, panes_error = mux_json("list", "--format", "json")
  if not remote or not panes then
    fail(window, "attach_mux: " .. (list_error or panes_error))
    return
  end

  local window_id
  local choices = {}
  for _, entry in ipairs(panes) do
    if entry.pane_id == source then
      window_id = entry.window_id
    end
    if entry.workspace == "__detached" and entry.is_active then
      table.insert(choices, { id = "detached:" .. entry.pane_id, label = describe(entry, "detached") })
    end
  end
  if not window_id then
    fail(window, "attach_mux: pane " .. source .. " is not in localmux")
    return
  end
  for _, entry in ipairs(remote) do
    local place = ("%s pane %d"):format(target, entry.pane_id)
    table.insert(choices, { id = "remote:" .. entry.pane_id, label = describe(entry, place) })
  end
  if #choices == 0 then
    window:toast_notification("attach_mux", "Nothing to adopt from " .. target, nil, 4000)
    return
  end

  window:perform_action(
    act.InputSelector {
      title = "Adopt a pane",
      fuzzy = true,
      choices = choices,
      action = wezterm.action_callback(function(selector_window, _, choice)
        if choice then
          show(selector_window, choice, domain, window_id)
        end
      end),
    },
    pane
  )
end

---@param action fun(window, pane, target: string)
local function once_per_pane(action)
  return function(window, pane, target)
    target = target == "peer" and host.target.hostname or target
    local id = pane:pane_id()
    if pending[id] then
      return
    end
    pending[id] = true
    local ok, reason = pcall(action, window, pane, target)
    pending[id] = nil
    if not ok then
      fail(window, "attach_mux: " .. tostring(reason))
    end
  end
end

local requests = {
  ATTACH_MUX = once_per_pane(replace),
  ADOPT_MUX = once_per_pane(adopt),
}

wezterm.on("user-var-changed", function(window, pane, name, value)
  local request = requests[name]
  local target = request and value:match "^v1:([%w._-]+):%d+:%d+%.%d+$"
  if target then
    request(window, pane, target)
  end
end)

return {
  adopt = wezterm.action_callback(function(window, pane)
    requests.ADOPT_MUX(window, pane, "peer")
  end),
}
