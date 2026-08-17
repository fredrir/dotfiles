local context = require 'wez.dmux_bridge.context'
local json = require 'wez.dmux_bridge.json'
local protocol = require 'wez.dmux_bridge.protocol'
local wezterm = require 'wezterm'

local M = {}
local active_bridge
local active_identity
local active_instance
local configured_persistent_domains
local configured_persistent_domain_instances

local UUID = '^[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]$'

local REQUIRED_CAPABILITIES = {
  'descriptor_backed_spool',
  'exclusive_instance_lease',
  'launcher_witness',
  'checked_preflight',
  'capability_bound_lifecycle_completion',
  'zero_window_lifecycle',
  'verified_mux_descriptor',
}

---Validate the maintained-fork surface without opening an instance lease.
---Production never falls back to caller-derived runtime paths.
function M.require_secure_surface()
  local gui = wezterm.gui
  if
    type(gui) ~= 'table'
    or type(gui.dmux_bridge_capabilities) ~= 'function'
    or type(gui.dmux_bridge_preflight) ~= 'function'
    or type(gui.dmux_bridge_open) ~= 'function'
  then
    return nil, 'maintained fork secure bridge surface is unavailable'
  end
  local ok, capabilities = pcall(gui.dmux_bridge_capabilities)
  if not ok or type(capabilities) ~= 'table' or capabilities.version ~= 1 then
    return nil, 'maintained fork bridge capabilities are unavailable or incompatible'
  end
  for _, capability in ipairs(REQUIRED_CAPABILITIES) do
    if capabilities[capability] ~= true then
      return nil, 'maintained fork bridge capability is missing: ' .. capability
    end
  end
  return gui
end

function M.checked_preflight()
  local gui, surface_err = M.require_secure_surface()
  if not gui then
    return nil, surface_err
  end
  local ok, proof = pcall(gui.dmux_bridge_preflight)
  if
    not ok
    or type(proof) ~= 'table'
    or proof.version ~= 1
    or proof.key_bytes ~= 32
    or proof.runtime_verified ~= true
    or proof.verified_mux_descriptor ~= true
    or type(proof.launcher_witness_present) ~= 'boolean'
  then
    return nil, 'maintained fork checked bridge preflight failed: ' .. tostring(proof)
  end
  return proof
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

local IDENTITY_KEYS = {
  gui_instance = true,
  pid = true,
  process_start_token = true,
}

---Freeze the final sanitized persistent-domain inventory in this config
---generation. `wezterm.GLOBAL` is process-shared presentation state, but it
---is not the authority handoff between config evaluation and gui-startup;
---the event callback and this module share the same reload-disabled Lua
---generation, so a defensive module-local snapshot is exact and bounded.
function M.configure_persistent_domains(persistent_domains, domain_instances)
  if
    type(persistent_domains) ~= 'table'
    or getmetatable(persistent_domains) ~= nil
    or type(domain_instances) ~= 'table'
    or getmetatable(domain_instances) ~= nil
  then
    return nil, 'managed persistent domain configuration is unavailable'
  end
  local copied_domains, copied_instances, expected = {}, {}, {}
  for index, name in ipairs(persistent_domains) do
    if
      type(name) ~= 'string'
      or #name == 0
      or name == 'local'
      or expected[name]
      or not name:match '^[A-Za-z0-9][A-Za-z0-9_.:-]*$'
    then
      return nil, 'managed persistent domain configuration is malformed'
    end
    local backend_instance_uid = domain_instances[name]
    if type(backend_instance_uid) ~= 'string' or not backend_instance_uid:match(UUID) then
      return nil, 'managed persistent domain has no canonical backend instance: ' .. name
    end
    copied_domains[index] = name
    copied_instances[name] = backend_instance_uid
    expected[name] = true
  end
  for key in pairs(persistent_domains) do
    if type(key) ~= 'number' or key < 1 or key % 1 ~= 0 or key > #persistent_domains then
      return nil, 'managed persistent domain configuration is not a dense array'
    end
  end
  for name in pairs(domain_instances) do
    if type(name) ~= 'string' or not expected[name] then
      return nil, 'managed persistent domain instance configuration has an unknown domain'
    end
  end
  configured_persistent_domains = copied_domains
  configured_persistent_domain_instances = copied_instances
  return true
end

local function valid_lease_identity(identity, gui_instance, pid, process_start_token)
  if type(identity) ~= 'table' or getmetatable(identity) ~= nil then
    return false
  end
  for key in pairs(identity) do
    if type(key) ~= 'string' or not IDENTITY_KEYS[key] then
      return false
    end
  end
  return identity.gui_instance == gui_instance
    and type(identity.pid) == 'number'
    and identity.pid % 1 == 0
    and identity.pid > 0
    and identity.pid <= 4294967295
    and (pid == nil or identity.pid == pid)
    and type(identity.process_start_token) == 'string'
    and #identity.process_start_token > 0
    and #identity.process_start_token <= 256
    and not identity.process_start_token:find '[%z\1-\31\127]'
    and (process_start_token == nil or identity.process_start_token == process_start_token)
end

function M.create()
  local pid = wezterm.procinfo.pid()
  local instance, instance_err = requested_instance(pid)
  if not instance then
    return nil, instance_err
  end
  local persistent_domains = configured_persistent_domains
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
  local configured_instances = configured_persistent_domain_instances
  if type(configured_instances) ~= 'table' then
    return nil, 'managed persistent domain instance inventory is unavailable'
  end
  local copied_instances = {}
  local expected = {}
  for _, name in ipairs(copied_domains) do
    expected[name] = true
    local backend_instance_uid = configured_instances[name]
    if type(backend_instance_uid) ~= 'string' or not backend_instance_uid:match(UUID) then
      return nil, 'managed persistent domain has no canonical backend instance: ' .. name
    end
    copied_instances[name] = backend_instance_uid
  end
  for name in pairs(configured_instances) do
    if type(name) ~= 'string' or not expected[name] then
      return nil, 'managed persistent domain instance inventory has an unknown domain'
    end
  end
  local gui, surface_err = M.require_secure_surface()
  if not gui then
    return nil, surface_err
  end
  local opened, bridge_or_err = pcall(gui.dmux_bridge_open, instance)
  if not opened or bridge_or_err == nil then
    return nil, 'cannot acquire exclusive maintained-fork bridge lease: ' .. tostring(bridge_or_err)
  end
  local bridge = bridge_or_err
  local identity_ok, identity = pcall(function()
    return bridge:identity()
  end)
  local key_ok, key = pcall(function()
    return bridge:key()
  end)
  if
    not identity_ok
    or not valid_lease_identity(identity, instance, pid)
    or not key_ok
    or type(key) ~= 'string'
    or #key ~= 32
  then
    return nil, 'maintained-fork bridge lease returned an invalid identity or key'
  end
  active_bridge = bridge
  active_identity = {
    gui_instance = identity.gui_instance,
    pid = identity.pid,
    process_start_token = identity.process_start_token,
  }
  active_instance = instance
  return {
    id = instance,
    key = key,
    pid = pid,
    process_start_token = identity.process_start_token,
    bridge = bridge,
    safe_quit = {},
    cold_launchers = {},
    persistent_domains = copied_domains,
    persistent_domain_instances = copied_instances,
  }
end

function M.current_bridge(gui_instance)
  if type(gui_instance) ~= 'string' or gui_instance ~= active_instance or active_bridge == nil then
    return nil, 'trusted dmux GUI bridge lease is not active for this instance'
  end
  return active_bridge
end

---Return a defensive copy of the identity held by the active fork lease.
---The lease is re-read so a caller never derives resident authority from a
---stale module global or an unverified process lookup.
function M.current_identity()
  if active_bridge == nil or active_identity == nil or active_instance == nil then
    return nil, 'trusted dmux GUI bridge lease is not active'
  end
  local ok, identity = pcall(function()
    return active_bridge:identity()
  end)
  if
    not ok
    or not valid_lease_identity(
      identity,
      active_identity.gui_instance,
      active_identity.pid,
      active_identity.process_start_token
    )
    or identity.gui_instance ~= active_instance
  then
    return nil, 'maintained-fork bridge lease identity changed or became unavailable'
  end
  return {
    gui_instance = active_identity.gui_instance,
    pid = active_identity.pid,
    process_start_token = active_identity.process_start_token,
  }
end

local function system_workspace(workspace)
  if type(workspace) ~= 'string' then
    return nil
  end
  local epoch = workspace:match '^dmux:system:(.+)$'
  return epoch and epoch:match(UUID) and epoch or nil
end

local function snapshot(state)
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
      backend_instance_uid = state.persistent_domain_instances[name],
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
        local system_epoch = system_workspace(workspace)
        if system_epoch then
          -- Rust accepts this exemption only after matching the exact owner
          -- descriptor/sentinel epoch. Lua merely reports the syntactic
          -- reserved workspace; it never treats it as owner authority.
          if counts.system_workspace and counts.system_workspace ~= workspace then
            return nil, 'GUI domain contains multiple system workspaces'
          end
          if counts.system_epoch and counts.system_epoch ~= system_epoch then
            return nil, 'GUI domain contains multiple system epochs'
          end
          counts.system_workspace = workspace
          counts.system_epoch = system_epoch
          counts.system_pane_count = counts.system_pane_count + 1
        else
          local marker = context.from_pane(pane)
          if marker then
            if seen[marker.gui_pane_id] then
              return nil, 'duplicate GUI pane id in heartbeat'
            end
            seen[marker.gui_pane_id] = true
            counts.valid_marker_pane_count = counts.valid_marker_pane_count + 1
            local heartbeat_pane = {
              pane_id = marker.gui_pane_id,
              domain = marker.gui_domain,
              context = context.marker_context(marker),
            }
            if marker.tmux_client_uid then
              heartbeat_pane.tmux_client_uid = marker.tmux_client_uid
            end
            table.insert(panes, heartbeat_pane)
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
  local panes, domains_or_err = snapshot(state)
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
  if state.bridge == nil or type(state.bridge.write_heartbeat_atomic) ~= 'function' then
    return nil, 'maintained-fork heartbeat writer is unavailable'
  end
  local ok, result = pcall(function()
    return state.bridge:write_heartbeat_atomic(body)
  end)
  if not ok then
    return nil, tostring(result)
  end
  return result == nil and true or result
end

return M
