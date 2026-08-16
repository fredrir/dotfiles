local context = require 'wez.dmux_bridge.context'
local fs = require 'wez.dmux_bridge.fs'
local json = require 'wez.dmux_bridge.json'
local protocol = require 'wez.dmux_bridge.protocol'
local wezterm = require 'wezterm'

local M = {}

function M.runtime_dir()
  local explicit = os.getenv 'DMUX_RUNTIME_DIR'
  if explicit and explicit:sub(1, 1) == '/' then
    local trimmed = explicit:gsub('/+$', '')
    if trimmed == '' then
      return nil, 'runtime directory cannot be the filesystem root'
    end
    return trimmed
  end
  local base
  if wezterm.target_triple:find 'darwin' then
    base = os.getenv 'TMPDIR'
  else
    base = os.getenv 'XDG_RUNTIME_DIR'
  end
  if not base or base:sub(1, 1) ~= '/' then
    return nil, 'no absolute platform runtime directory'
  end
  local trimmed = base:gsub('/+$', '')
  if trimmed == '' then
    return nil, 'platform runtime base cannot be the filesystem root'
  end
  return trimmed .. '/dmux'
end

local function random_hex(bytes)
  local file = io.open('/dev/urandom', 'rb')
  if not file then
    return nil
  end
  local value = file:read(bytes)
  file:close()
  if not value or #value ~= bytes then
    return nil
  end
  return (value:gsub('.', function(char)
    return string.format('%02x', char:byte())
  end))
end

local function process_start_token(pid)
  local ok, success, stdout = pcall(wezterm.run_child_process, {
    '/usr/bin/env',
    'LC_ALL=C',
    '/bin/ps',
    '-p',
    tostring(pid),
    '-o',
    'lstart=',
  })
  if not ok or not success then
    return nil
  end
  local token = stdout:gsub('^%s+', ''):gsub('%s+$', '')
  return #token > 0 and token or nil
end

local function requested_instance(pid)
  local requested = os.getenv 'DMUX_GUI_INSTANCE'
  if requested then
    if #requested > 160 or not requested:match '^[A-Za-z0-9][A-Za-z0-9_-]+$' then
      return nil, 'DMUX_GUI_INSTANCE is malformed'
    end
    return requested
  end
  local nonce = random_hex(16)
  if not nonce then
    return nil, 'cannot read /dev/urandom for GUI instance id'
  end
  return string.format('gui-%d-%s', pid, nonce)
end

function M.create()
  local runtime, runtime_err = M.runtime_dir()
  if not runtime then
    return nil, runtime_err
  end
  local bridge = fs.join(runtime, 'bridge')
  local key, key_err = fs.read(fs.join(bridge, 'key'), 33)
  if not key then
    return nil, 'bridge key unavailable: ' .. tostring(key_err)
  end
  if #key ~= 32 then
    return nil, 'bridge key must contain exactly 32 raw bytes'
  end

  local pid = wezterm.procinfo.pid()
  local instance, instance_err = requested_instance(pid)
  if not instance then
    return nil, instance_err
  end
  local start_token = process_start_token(pid)
  if not start_token then
    return nil, 'cannot determine GUI process start token'
  end
  local dir = fs.join(bridge, 'instances', instance)
  local paths = {
    dir = dir,
    requests = fs.join(dir, 'requests'),
    acks = fs.join(dir, 'acks'),
    consumed = fs.join(dir, 'consumed'),
    heartbeat = fs.join(dir, 'heartbeat.json'),
  }
  local ok, err =
    fs.ensure_private_dirs { bridge, fs.join(bridge, 'instances'), dir, paths.requests, paths.acks, paths.consumed }
  if not ok then
    return nil, err
  end
  local persistent_domains = wezterm.GLOBAL.dmux_managed_persistent_domains
  if type(persistent_domains) ~= 'table' then
    return nil, 'managed persistent domain inventory is unavailable'
  end
  local copied_domains = {}
  for index, name in ipairs(persistent_domains) do
    if type(name) ~= 'string' or #name == 0 or name == 'local' then
      return nil, 'managed persistent domain inventory is malformed'
    end
    copied_domains[index] = name
  end
  for key_name in pairs(persistent_domains) do
    if type(key_name) ~= 'number' or key_name < 1 or key_name % 1 ~= 0 or key_name > #persistent_domains then
      return nil, 'managed persistent domain inventory is not a dense array'
    end
  end
  return {
    id = instance,
    key = key,
    pid = pid,
    process_start_token = start_token,
    paths = paths,
    safe_quit = {},
    persistent_domains = copied_domains,
  }
end

local UUID = '^[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]$'

local function system_workspace(workspace)
  if type(workspace) ~= 'string' then
    return false
  end
  local epoch = workspace:match '^dmux:system:(.+)$'
  return epoch ~= nil and epoch:match(UUID) ~= nil
end

local function snapshot()
  local domains = {}
  for _, domain in ipairs(wezterm.mux.all_domains()) do
    local name_ok, name = pcall(function()
      return domain:name()
    end)
    if
      not name_ok
      or type(name) ~= 'string'
      or #name == 0
      or #name > 128
      or not name:match '^[A-Za-z0-9][A-Za-z0-9_.:-]*$'
    then
      return nil, 'GUI domain name is unavailable or malformed'
    end
    if domains[name] then
      return nil, 'duplicate GUI domain name in heartbeat: ' .. name
    end
    local state_ok, domain_state = pcall(function()
      return domain:state()
    end)
    local panes_ok, has_panes = pcall(function()
      return domain:has_any_panes()
    end)
    if
      not state_ok
      or type(domain_state) ~= 'string'
      or #domain_state == 0
      or #domain_state > 64
      or not domain_state:match '^[A-Za-z]+$'
      or not panes_ok
      or type(has_panes) ~= 'boolean'
    then
      return nil, 'GUI domain state is unavailable or malformed: ' .. name
    end
    domains[name] = {
      state = domain_state,
      has_any_panes = has_panes,
      pane_count = 0,
      valid_marker_pane_count = 0,
      system_pane_count = 0,
    }
  end

  local panes = json.array()
  local seen = {}
  for _, window in ipairs(wezterm.mux.all_windows()) do
    local workspace_ok, workspace = pcall(function()
      return window:get_workspace()
    end)
    workspace = workspace_ok and workspace or nil
    for _, tab in ipairs(window:tabs()) do
      for _, pane in ipairs(tab:panes()) do
        local domain_ok, domain_name = pcall(function()
          return pane:get_domain_name()
        end)
        if not domain_ok or type(domain_name) ~= 'string' or not domains[domain_name] then
          return nil, 'GUI pane belongs to an unknown domain'
        end
        local counts = domains[domain_name]
        counts.pane_count = counts.pane_count + 1
        if system_workspace(workspace) then
          -- Rust accepts this exemption only after matching the exact owner
          -- descriptor/sentinel epoch. Lua merely reports the syntactic
          -- reserved workspace; it never treats it as owner authority.
          counts.system_pane_count = counts.system_pane_count + 1
        else
          local marker = context.from_pane(pane)
          if marker then
            if seen[marker.gui_pane_id] then
              return nil, 'duplicate GUI pane id in heartbeat'
            end
            seen[marker.gui_pane_id] = true
            counts.valid_marker_pane_count = counts.valid_marker_pane_count + 1
            table.insert(panes, {
              pane_id = marker.gui_pane_id,
              domain = marker.gui_domain,
              context = context.marker_context(marker),
            })
          end
          -- An invalid marker intentionally increments only pane_count. The
          -- resulting inequality is a durable fail-closed coverage witness.
        end
      end
    end
  end
  table.sort(panes, function(left, right)
    return left.pane_id < right.pane_id
  end)
  return panes, domains
end

function M.heartbeat(state)
  local panes, domains_or_err = snapshot()
  if not panes then
    return nil, domains_or_err
  end
  local body, encode_err = json.encode {
    protocol_version = protocol.VERSION,
    gui_instance = state.id,
    pid = state.pid,
    process_start_token = state.process_start_token,
    updated_at = os.time(),
    panes = panes,
    domains = domains_or_err,
  }
  if not body then
    return nil, 'heartbeat encoding failed: ' .. tostring(encode_err)
  end
  if #body > protocol.MAX_DOCUMENT_BYTES then
    return nil, 'heartbeat exceeds the bridge document limit'
  end
  return fs.write_private_atomic(state.paths.heartbeat, body)
end

return M
