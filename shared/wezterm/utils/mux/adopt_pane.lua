local wezterm = require "wezterm" ---@type Wezterm
local act = wezterm.action
local dotfile = require "utils.dotfile"
local hwire_session = require "utils.hwire-session"
local str = require "utils.str"
local is_remote = require("utils.remote").is_remote
local host = require "domain.hosts"
local ssh_hosts = require "domain.ssh-hosts"
local ssh_mux = require "domain.ssh-mux"

local MUX_ROUTE = dotfile.compiled_dir .. "/mux-route"
local SOCKET = require("utils.mux.mux").localmux_socket
local CLI = wezterm.executable_dir .. "/wezterm"
local MUX_TIMEOUT_SECONDS = 15
local ISOLATED = 'exec /usr/bin/env -i WEZTERM_PANE="$WEZTERM_PANE" WEZTERM_UNIX_SOCKET="$WEZTERM_UNIX_SOCKET" "$@"'
local pending = {}

---@return string?, string?
local function mux(...)
  local args = {
    "/usr/bin/perl",
    "-e",
    "alarm shift; exec @ARGV or die $!",
    tostring(MUX_TIMEOUT_SECONDS),
    "/usr/bin/env",
    "WEZTERM_UNIX_SOCKET=" .. SOCKET,
    CLI,
    "cli",
    "--prefer-mux",
    "--no-auto-start",
  }
  for _, arg in ipairs { ... } do
    table.insert(args, tostring(arg))
  end
  local ok, stdout, stderr = wezterm.run_child_process(args)
  if ok then
    return stdout
  end
  local message = str.trim(stderr)
  return nil,
    message ~= "" and message or ("localmux: unreachable; in %ds; run \nwez-restart"):format(MUX_TIMEOUT_SECONDS)
end

---@return table?, string?
local function mux_json(...)
  local stdout, err = mux(...)
  return stdout and wezterm.json_parse(stdout), err
end

---@class AttachTarget
---@field domain string
---@field home string? isolated login zsh in `home`; the domain's default shell when nil
---@field session string?
---@field adoptable boolean?

---@return AttachTarget?, string?
local function resolve(target)
  if target == host.origin.hostname then
    return { domain = "local", home = host.origin.home }
  end

  if target == host.target.hostname then
    local ok, stdout, stderr = wezterm.run_child_process { MUX_ROUTE, target }
    if not ok then
      return nil, "mux-route: " .. str.trim(stderr)
    end
    local domain = str.trim(stdout)
    local session = hwire_session.for_domain(domain)
    if not session then
      return nil, "mux-route: invalid domain"
    end
    return { domain = domain, home = host.target.home, session = session, adoptable = true }
  end

  local remote = ssh_mux.hosts[target]
  if remote then
    if not ssh_mux.supported then
      return nil, target .. " needs a WezTerm build with unix local_pane_layout"
    end
    return { domain = target, home = remote.home, adoptable = true }
  end

  for _, name in ipairs(ssh_hosts()) do
    if name == target then
      return { domain = target }
    end
  end

  return nil, "unknown host: " .. target
end

---@param to AttachTarget
---@return string[]
local function spawn_args(to)
  if not to.home then
    return { "--domain-name", to.domain }
  end
  return {
    "--domain-name",
    to.domain,
    "--cwd",
    to.home,
    "--",
    "/bin/sh",
    "-c",
    ISOLATED,
    "attach_mux",
    "HOME=" .. to.home,
    "TERM=xterm-256color",
    "COLORTERM=truecolor",
    "PATH=/usr/local/bin:/usr/bin:/bin",
    "HWIRE_SESSION=" .. (to.session or ""),
    "zsh",
    "-l",
  }
end

---@return integer?, string? localmux pane ID behind the GUI pane
local function localmux_pane(pane)
  local metadata = pane:get_metadata() or {}
  local source = metadata.remote_pane_id
  if pane:get_domain_name() ~= "localmux" or type(source) ~= "number" then
    return nil, "requires localmux and the updated WezTerm build"
  end
  return source
end

---@param done fun(err: string?)
local function replace(_, pane, source, target, done)
  if target == "toggle" then
    target = is_remote(pane) and host.origin.hostname or host.target.hostname
  end
  local to, err = resolve(target)
  if not to then
    return done(err)
  end

  local tab = pane:tab()
  local stdout, split_error = mux("split-pane", "--pane-id", source, table.unpack(spawn_args(to)))
  if not stdout then
    return done(split_error)
  end
  local replacement = tonumber(stdout:match "^%s*(%d+)%s*$")
  if not replacement or replacement == source then
    return done "invalid replacement pane"
  end
  local focused, focus_error = pcall(function()
    for _ = 1, 100 do
      for _, candidate in ipairs(tab:panes()) do
        local metadata = candidate:get_metadata() or {}
        if candidate:get_domain_name() == "localmux" and metadata.remote_pane_id == replacement then
          candidate:activate()
          return
        end
      end
      wezterm.sleep_ms(50)
    end
    error "replacement pane did not appear in the GUI"
  end)
  if not focused then
    mux("kill-pane", "--pane-id", replacement)
    return done(tostring(focus_error))
  end
  local closed, close_error = mux("kill-pane", "--pane-id", source)
  if not closed then
    mux("kill-pane", "--pane-id", replacement)
    return done(close_error)
  end
  done()
end

---@param done fun(err: string?)
local function new_tab(_, _, source, target, done)
  local to, err = resolve(target)
  if not to then
    return done(err)
  end

  local stdout, spawn_error = mux("spawn", "--pane-id", source, table.unpack(spawn_args(to)))
  if not stdout then
    return done(spawn_error)
  end
  local created = tonumber(stdout:match "^%s*(%d+)%s*$")
  if not created then
    return done "invalid new pane"
  end
  local _, activate_error = mux("activate-pane", "--pane-id", created)
  done(activate_error)
end

local function describe(entry, place)
  local cwd = (entry.cwd or ""):gsub("^file://[^/]*", "")
  return ("%s  %s  (%s)"):format(entry.title, cwd, place)
end

---@return string?
local function show(choice, domain, window_id)
  local kind, pane_id = choice:match "^(%a+):(%d+)$"
  local detached = kind == "detached"
  local args = detached and { "move-pane-to-new-tab", "--pane-id", pane_id, "--window-id", window_id }
    or { "adopt-pane", "--domain-name", domain, "--remote-pane-id", pane_id, "--window-id", window_id }
  local stdout, err = mux(table.unpack(args))
  if not stdout then
    return err
  end
  mux("activate-pane", "--pane-id", detached and pane_id or str.trim(stdout))
end

---@param done fun(err: string?)
local function adopt(window, pane, source, target, done)
  local to, err = resolve(target)
  if not to then
    return done(err)
  end
  if not to.adoptable then
    return done(target .. " has no mux to adopt from")
  end
  local remote, list_error = mux_json("adopt-pane", "--domain-name", to.domain, "--list", "--format", "json")
  local panes, panes_error = mux_json("list", "--format", "json")
  if not remote or not panes then
    return done(list_error or panes_error)
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
    return done("pane " .. source .. " is not in localmux")
  end
  for _, entry in ipairs(remote) do
    local place = ("%s pane %d"):format(target, entry.pane_id)
    table.insert(choices, { id = "remote:" .. entry.pane_id, label = describe(entry, place) })
  end
  if #choices == 0 then
    return done("nothing to adopt from " .. target)
  end

  window:perform_action(
    act.InputSelector {
      title = "Adopt a pane",
      fuzzy = true,
      choices = choices,
      action = wezterm.action_callback(function(_, _, choice)
        local show_error = choice and show(choice, to.domain, window_id)
        if show_error then
          wezterm.log_error("attach_mux: " .. show_error)
        end
      end),
    },
    pane
  )
  done()
end

---@param id string?
---@param message string?
local function reply(pane, id, message)
  if id then
    pcall(pane.send_text, pane, ("%s %s\a"):format(id, ((message or ""):gsub("%c", " "))))
  end
end

---@param action fun(window, pane, source: integer, target: string, done: fun(err: string?))
local function once_per_pane(action)
  return function(window, pane, target, id)
    target = target == "peer" and host.target.hostname or target
    local pane_id = pane:pane_id()
    if pending[pane_id] then
      return reply(pane, id, "attach_mux: request already pending")
    end
    pending[pane_id] = true
    local function done(err)
      pending[pane_id] = nil
      local message = err and "attach_mux: " .. err
      if message then
        wezterm.log_error(message)
      end
      reply(pane, id, message)
    end
    local source, source_error = localmux_pane(pane)
    if not source then
      return done(source_error)
    end
    local ok, err = pcall(action, window, pane, source, target, done)
    if not ok then
      done(tostring(err))
    end
  end
end

local requests = {
  ATTACH_MUX = once_per_pane(replace),
  ADOPT_MUX = once_per_pane(adopt),
}

wezterm.on("user-var-changed", function(window, pane, name, value)
  local request = requests[name]
  if not request then
    return
  end
  local target, id = value:match "^v1:([%w._-]+):(%d+:%d+%.%d+)$"
  if target then
    request(window, pane, target, id)
  end
end)

local open_peer_tab = once_per_pane(new_tab)

return {
  toggle_host = wezterm.action_callback(function(window, pane)
    requests.ATTACH_MUX(window, pane, "toggle")
  end),
  new_peer_tab = wezterm.action_callback(function(window, pane)
    open_peer_tab(window, pane, "peer")
  end),
  adopt = wezterm.action_callback(function(window, pane)
    requests.ADOPT_MUX(window, pane, "peer")
  end),
}