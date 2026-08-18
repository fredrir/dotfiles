local wezterm = require 'wezterm'
local act = wezterm.action
local platform = require 'wez.platform'
local json = require 'wez.dmux_bridge.json'

-- Native mux to the peer machine. Both ssh domains reach the same
-- wezterm-mux-server, so windows and panes are identical through either
-- path: when the cable dies mid-session, attaching the -ts domain resumes
-- exactly where the -usb session stopped.
local M = {}
local managed_persistent_domain_instances
local managed_persistent_domain_owners

local function dmux_enabled()
  return os.getenv 'DMUX_WEZ_FIRST' == '1'
end

local UUID = '^[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]$'

local DOMAIN_ROW_KEYS = {
  alternate_domains = true,
  backend_instance_uid = true,
  compatible = true,
  host_uid = true,
  name = true,
  network_class = true,
  priority = true,
  remote_address = true,
  remote_wezterm_path = true,
  route_id = true,
  transport = true,
  unavailable_reason = true,
  username = true,
}

local function exact_keys(value, allowed)
  if type(value) ~= 'table' then
    return false
  end
  for key in pairs(value) do
    if type(key) ~= 'string' or not allowed[key] then
      return false
    end
  end
  return true
end

local function bounded_string(value, maximum)
  return type(value) == 'string' and #value > 0 and #value <= maximum and not value:find '[%z\1-\31\127]'
end

local function valid_domain(value)
  return bounded_string(value, 128) and value:match '^[A-Za-z0-9][A-Za-z0-9_.:-]*$' ~= nil
end

local function valid_integer(value)
  -- Two-sided, not math.abs: abs(math.mininteger) is itself and would pass.
  return type(value) == 'number' and value % 1 == 0 and value >= -9007199254740991 and value <= 9007199254740991
end

local function dense_array(value, validator)
  if type(value) ~= 'table' or not json.is_array(value) then
    return false
  end
  for index, item in ipairs(value) do
    if not validator(item, index) then
      return false
    end
  end
  for key in pairs(value) do
    if type(key) ~= 'number' or key % 1 ~= 0 or key < 1 or key > #value then
      return false
    end
  end
  return true
end

local function valid_manifest_row(row)
  if not exact_keys(row, DOMAIN_ROW_KEYS) then
    return false
  end
  local transport = { openssh = true, ['wez-ssh'] = true }
  local network_class = { usb = true, tailscale = true, lan = true, other = true }
  if
    not valid_domain(row.name)
    or not bounded_string(row.remote_address, 1024)
    or not bounded_string(row.username, 256)
    or type(row.host_uid) ~= 'string'
    or not row.host_uid:match(UUID)
    or type(row.backend_instance_uid) ~= 'string'
    or not row.backend_instance_uid:match(UUID)
    or not valid_integer(row.route_id)
    or row.route_id <= 0
    or not valid_integer(row.priority)
    or not transport[row.transport]
    or not network_class[row.network_class]
    or type(row.compatible) ~= 'boolean'
    or not dense_array(row.alternate_domains, function(name)
      return valid_domain(name)
    end)
  then
    return false
  end
  if
    row.remote_wezterm_path ~= nil
    and (not bounded_string(row.remote_wezterm_path, 1024) or row.remote_wezterm_path:sub(1, 1) ~= '/')
  then
    return false
  end
  if row.compatible then
    return row.remote_wezterm_path ~= nil and row.unavailable_reason == nil
  end
  return bounded_string(row.unavailable_reason, 4096)
end

local function validate_manifest(rows)
  if not dense_array(rows, valid_manifest_row) then
    return false
  end
  local names, route_ids = {}, {}
  local instance_by_host, host_by_instance = {}, {}
  for _, row in ipairs(rows) do
    if names[row.name] or route_ids[row.route_id] then
      return false
    end
    local known_instance = instance_by_host[row.host_uid]
    local known_host = host_by_instance[row.backend_instance_uid]
    if
      (known_instance and known_instance ~= row.backend_instance_uid)
      or (known_host and known_host ~= row.host_uid)
    then
      return false
    end
    instance_by_host[row.host_uid] = row.backend_instance_uid
    host_by_instance[row.backend_instance_uid] = row.host_uid
    names[row.name], route_ids[row.route_id] = true, true
    local seen = {}
    for _, alternate in ipairs(row.alternate_domains) do
      if alternate == row.name or seen[alternate] then
        return false
      end
      seen[alternate] = true
    end
  end
  for _, row in ipairs(rows) do
    local expected = {}
    for _, other in ipairs(rows) do
      if
        other.name ~= row.name
        and other.host_uid == row.host_uid
        and other.backend_instance_uid == row.backend_instance_uid
        and other.compatible
      then
        table.insert(expected, other.name)
      end
    end
    if #expected ~= #row.alternate_domains then
      return false
    end
    for index, name in ipairs(expected) do
      if row.alternate_domains[index] ~= name then
        return false
      end
    end
  end
  return true
end

local PEER = platform.pick {
  mac = {
    name = 'archie',
    usb_address = '10.77.77.2',
    ts_address = '100.126.231.24',
    wezterm_path = '/usr/bin/wezterm',
    -- Mirrors the probe in macos/ssh/config.d/05-archie-cabled-first.
    probe = { '/usr/bin/nc', '-4', '-z', '-G', '1', '-b', 'en3', '10.77.77.2', '22' },
  },
  linux = {
    name = 'macie',
    usb_address = '10.77.77.1',
    ts_address = '100.75.71.79',
    wezterm_path = '/opt/homebrew/bin/wezterm',
    -- Mirrors the probe in linux/arch/ssh/config.d/05-macie-cabled-first.
    probe = { '/usr/bin/nc', '-z', '-w', '1', '-s', '10.77.77.2', '10.77.77.1', '22' },
  },
}

local function domain_name(suffix)
  return PEER.name .. '-' .. suffix
end

function M.domains()
  if dmux_enabled() then
    -- Clear first so a reload which loses/mangles the authority response can
    -- never retain identities from the preceding successful evaluation.
    managed_persistent_domain_instances = {}
    managed_persistent_domain_owners = {}
    local bin = os.getenv 'DMUX_BIN' or (wezterm.home_dir .. '/.local/bin/dmux')
    local spawned, ok, stdout, stderr = pcall(wezterm.run_child_process, { bin, '_gui', 'domains' })
    if not spawned or not ok then
      wezterm.log_error('dmux domain manifest unavailable: ' .. tostring(spawned and stderr or ok))
      return {}
    end
    if type(stdout) ~= 'string' or #stdout > 64 * 1024 then
      wezterm.log_error 'dmux domain manifest exceeds the bounded response size'
      return {}
    end
    local response = json.decode(stdout)
    if
      type(response) ~= 'table'
      or not exact_keys(response, { schema_version = true, ok = true, result = true })
      or response.schema_version ~= 1
      or response.ok ~= true
      or type(response.result) ~= 'table'
      or not exact_keys(response.result, { domains = true })
      or type(response.result.domains) ~= 'table'
      or not validate_manifest(response.result.domains)
    then
      wezterm.log_error 'dmux domain manifest is malformed'
      return {}
    end
    local domains = {}
    local instances = {}
    local owners = {}
    for _, row in ipairs(response.result.domains) do
      if row.compatible then
        table.insert(domains, {
          name = row.name,
          remote_address = row.remote_address,
          username = row.username,
          multiplexing = 'WezTerm',
          remote_wezterm_path = row.remote_wezterm_path,
          assume_shell = 'Posix',
        })
        instances[row.name] = row.backend_instance_uid
        owners[row.name] = row.host_uid
      else
        wezterm.log_warn(string.format('dmux GUI domain %s unavailable: %s', row.name, row.unavailable_reason))
      end
    end
    managed_persistent_domain_instances = instances
    managed_persistent_domain_owners = owners
    return domains
  end
  managed_persistent_domain_instances = nil
  managed_persistent_domain_owners = nil
  if not PEER then
    return {}
  end
  local function domain(suffix, address)
    return {
      name = domain_name(suffix),
      remote_address = address,
      username = 'fredrir',
      multiplexing = 'WezTerm',
      remote_wezterm_path = PEER.wezterm_path,
      assume_shell = 'Posix',
    }
  end
  -- Explicit addresses rather than ~/.ssh/config aliases: wezterm's built-in
  -- ssh client parses only a subset of that config, and the cabled-first
  -- `Match exec` probe is not in the subset. Path selection happens in
  -- attach_action instead.
  return { domain('usb', PEER.usb_address), domain('ts', PEER.ts_address) }
end

local function copied_map(source)
  if type(source) ~= 'table' then
    return nil
  end
  local copied = {}
  for name, value in pairs(source) do
    copied[name] = value
  end
  return copied
end

function M.managed_persistent_domain_instances()
  return copied_map(managed_persistent_domain_instances)
end

function M.managed_persistent_domain_owners()
  return copied_map(managed_persistent_domain_owners)
end

function M.usb_reachable()
  if dmux_enabled() then
    return false
  end
  if not PEER then
    return false
  end
  local ok, success = pcall(wezterm.run_child_process, PEER.probe)
  return ok and success
end

-- Attach the peer through the USB link when it answers, Tailscale otherwise.
-- Spawning a tab in the domain attaches it, which also brings along every
-- window already live on the remote mux server.
function M.attach_action()
  if dmux_enabled() then
    return require('wez.plugins.workspace_picker').action()
  end
  return wezterm.action_callback(function(window, pane)
    if not PEER then
      return
    end
    local name = domain_name(M.usb_reachable() and 'usb' or 'ts')
    window:perform_action(act.SpawnCommandInNewTab { domain = { DomainName = name } }, pane)
  end)
end

return M
