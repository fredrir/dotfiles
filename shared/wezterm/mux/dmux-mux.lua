-- dmux managed mux-server config (plan §15.1, ADR 002 frozen shape).
--
-- Loaded ONLY by `wezterm-mux-server --config-file <this file>`, started by
-- the service wrapper `dmux-mux-start.sh` under launchd (macOS) or systemd
-- (Linux). Never require this from the GUI config tree, and never start this
-- server by hand while the service is loaded: a second server on the same
-- socket path silently steals it and orphans the original (ADR 002).
--
-- Every input is injected by the wrapper's environment:
--   DMUX_SOCKET            exact unix socket to serve (short path, <104 bytes)
--   DMUX_RUNTIME_DIR       per-user runtime dir holding socket/logs
--   DMUX_SERVER_EPOCH      fresh UUID per server start
--   DMUX_BOOT_NONCE        fresh UUID per server start
--   DMUX_BACKEND_INSTANCE  durable backend-instance UID
--   DMUX_BIN               absolute dmux binary providing `_mux-idle`

local wezterm = require 'wezterm'

local SOCK = os.getenv 'DMUX_SOCKET'
local RUNTIME = os.getenv 'DMUX_RUNTIME_DIR'
local WEZ_FIRST = os.getenv 'DMUX_WEZ_FIRST' == '1'
local RECOVERY_PROTOCOL_VERSION = 1
local RESURRECT_URL = 'https://github.com/fredrir/resurrect.wezterm'
local owner_resurrect_dmux = nil
local owner_recovery_context = nil
local owner_recovery_generation_uid = nil
local owner_service_descriptor = nil
local owner_recovery_spool = nil
local owner_recovery_manifest = nil
local snapshot_serial = 0
local MAX_RECOVERY_MESSAGE_BYTES = 1024 * 1024
local MAX_RECOVERY_MANIFEST_BYTES = 16 * 1024 * 1024

-- `wezterm.background_child_process` injects the current mux endpoint into
-- its child environment even when the service wrapper scrubbed it before
-- starting the server.  Recovery helpers are deliberately registry/file
-- only and reject every inherited pane or mux identity, so interpose the
-- fixed system `env` binary for each helper launch.  Both supported owners
-- (macOS and Arch Linux) provide `/usr/bin/env` with `-u`.
local function registry_only_argv(argv)
  local clean = {
    '/usr/bin/env',
    '-u',
    'WEZTERM_UNIX_SOCKET',
    '-u',
    'WEZTERM_PANE',
    '-u',
    'TMUX',
    '-u',
    'TMUX_PANE',
  }
  for _, arg in ipairs(argv) do
    table.insert(clean, arg)
  end
  return clean
end
local schedule_guarded_snapshot

---@class DmuxRecoveryCommand
---@field protocol_version number
---@field coordinator_uid string
---@field generation_uid string
---@field sequence number
---@field fencing_token number
---@field action string
---@field nodes table[]|nil
---@field node table|nil
---@field request_uid string|nil
---@field bootstrap_argv string[]|nil
---@field expected_tree table|nil
---@field expected_parent string|nil
---@field expected_existing table|nil
---@field create_if_absent boolean|nil
---@field manifest_node_path string|nil
---@field pane_id string|nil
---@field tab_id string|nil
---@field window_id string|nil

-- Establish the native fixed-runtime capability during config evaluation,
-- before the mux listener is created. The maintained fork refuses to bind a
-- service socket without this retained capability. Never throw here: a Lua
-- config error can cause WezTerm to fall back to its default endpoint. A
-- missing/mismatched bootstrap instead configures a deliberate path mismatch,
-- which the native listener rejects before bind.
local MISCONFIGURED = SOCK == nil or RUNTIME == nil
local SERVICE_BOOTSTRAP_ERROR = nil
if RUNTIME == nil then
  RUNTIME = '/tmp'
end
if type(wezterm.mux.dmux_service_bootstrap) ~= 'function' then
  MISCONFIGURED = true
  SERVICE_BOOTSTRAP_ERROR = 'native managed-service bootstrap is unavailable'
else
  local ok, bootstrap = pcall(wezterm.mux.dmux_service_bootstrap)
  if
    not ok
    or type(bootstrap) ~= 'table'
    or bootstrap.api_version ~= 1
    or bootstrap.runtime_dir ~= RUNTIME
    or bootstrap.socket_path ~= SOCK
  then
    MISCONFIGURED = true
    SERVICE_BOOTSTRAP_ERROR = ok and 'native service bootstrap disagrees with configured fixed paths'
      or tostring(bootstrap)
  end
end
if MISCONFIGURED then
  SOCK = '/tmp/dmux-wez-misconfigured.sock'
end

local function now()
  return os.date '!%Y-%m-%dT%H:%M:%SZ'
end

local function proc_pid()
  if wezterm.procinfo and wezterm.procinfo.pid then
    local ok, pid = pcall(wezterm.procinfo.pid)
    if ok then
      return tonumber(pid) or -1
    end
  end
  return -1
end

local function log(msg)
  -- The service manager already captures stderr. Do not reopen a predictable
  -- runtime pathname from Lua: even a non-authority log append must not
  -- follow a same-UID symlink or race a retained native directory handle.
  wezterm.log_info(string.format('%s [pid=%d] %s', now(), proc_pid(), msg))
end

local DESCRIPTOR_OPTIONAL_FIELDS = {
  'sentinel_window_id',
  'sentinel_tab_id',
  'sentinel_pane_id',
  'sentinel_fallback',
  'recovery_generation',
  'recovery_manifest_id',
  'error',
}

-- Descriptor identity is a native service witness, not a Lua/environment
-- claim. The maintained fork resolves the fixed runtime path, obtains the
-- current OS boot/process/socket identities, checks the socket peer, and
-- durably publishes the closed version-1 document. It never accepts a path,
-- PID, process token, boot ID, or socket identity from this config.
local function publish_descriptor(state, fields)
  if type(wezterm.mux.dmux_publish_service_descriptor) ~= 'function' then
    return nil, 'native managed-service descriptor publisher is unavailable'
  end
  local request = {
    state = state,
    epoch = os.getenv 'DMUX_SERVER_EPOCH' or '',
    boot_nonce = os.getenv 'DMUX_BOOT_NONCE' or '',
  }
  local backend_instance = os.getenv 'DMUX_BACKEND_INSTANCE'
  if backend_instance and backend_instance ~= '' then
    request.backend_instance_uid = backend_instance
  end
  for _, key in ipairs(DESCRIPTOR_OPTIONAL_FIELDS) do
    if fields and fields[key] ~= nil then
      request[key] = fields[key]
    end
  end
  local ok, descriptor, raw = pcall(wezterm.mux.dmux_publish_service_descriptor, request)
  if not ok then
    return nil, tostring(descriptor)
  end
  local pid = proc_pid()
  if
    type(descriptor) ~= 'table'
    or type(raw) ~= 'string'
    or descriptor.descriptor_version ~= 1
    or descriptor.state ~= state
    or descriptor.epoch ~= request.epoch
    or descriptor.backend_instance_uid ~= request.backend_instance_uid
    or descriptor.boot_nonce ~= request.boot_nonce
    or descriptor.pid ~= pid
    or descriptor.socket ~= SOCK
    or descriptor.written_by ~= 'mux-startup'
    or type(descriptor.start_token) ~= 'string'
    or descriptor.start_token == ''
    or type(descriptor.boot_id) ~= 'string'
    or descriptor.boot_id == ''
  then
    return nil, 'native descriptor publisher returned a mismatched service witness'
  end
  if descriptor.peer_pid ~= nil and descriptor.peer_pid ~= pid then
    return nil, 'native descriptor publisher returned a foreign socket peer'
  end
  owner_service_descriptor = descriptor
  return descriptor
end

local function decode_json(raw, label)
  if raw == nil then
    return nil
  end
  if type(raw) ~= 'string' then
    return nil, tostring(label) .. ' native read returned non-string bytes'
  end
  local ok, value = pcall(wezterm.json_parse, raw)
  if not ok or type(value) ~= 'table' then
    return nil, 'cannot decode ' .. tostring(label) .. ': ' .. tostring(value)
  end
  return value
end

local function encode_json(value, label)
  local ok, encoded = pcall(wezterm.json_encode, value)
  if not ok then
    return nil, 'cannot encode ' .. tostring(label) .. ': ' .. tostring(encoded)
  end
  return encoded .. '\n'
end

local function read_spool_json(kind)
  if owner_recovery_spool == nil then
    return nil, 'native recovery spool handle is unavailable'
  end
  local ok, raw = pcall(function()
    return owner_recovery_spool:read(kind, MAX_RECOVERY_MESSAGE_BYTES)
  end)
  if not ok then
    return nil, 'cannot read recovery ' .. tostring(kind) .. ': ' .. tostring(raw)
  end
  return decode_json(raw, 'recovery ' .. tostring(kind))
end

local function write_spool_json(kind, value)
  if owner_recovery_spool == nil then
    return false, 'native recovery spool handle is unavailable'
  end
  local raw, encode_error = encode_json(value, 'recovery ' .. tostring(kind))
  if not raw then
    return false, encode_error
  end
  local ok, write_error = pcall(function()
    return owner_recovery_spool:write(kind, raw)
  end)
  if not ok then
    return false, 'cannot publish recovery ' .. tostring(kind) .. ': ' .. tostring(write_error)
  end
  return true
end

local function remove_spool_message(kind)
  if owner_recovery_spool == nil then
    return false, 'native recovery spool handle is unavailable'
  end
  local ok, removed = pcall(function()
    return owner_recovery_spool:remove(kind)
  end)
  if not ok then
    return false, 'cannot remove recovery ' .. tostring(kind) .. ': ' .. tostring(removed)
  end
  if removed ~= true then
    return false, 'recovery ' .. tostring(kind) .. ' disappeared before consumption'
  end
  return true
end

local function read_manifest_json(candidate_id, kind)
  if owner_recovery_manifest == nil then
    return nil, 'native recovery manifest handle is unavailable'
  end
  local ok, raw = pcall(function()
    return owner_recovery_manifest:read(candidate_id, kind, MAX_RECOVERY_MANIFEST_BYTES)
  end)
  if not ok then
    return nil, 'cannot read snapshot ' .. tostring(kind) .. ': ' .. tostring(raw)
  end
  return decode_json(raw, 'snapshot ' .. tostring(kind))
end

-- WezTerm's Lua JSON encoder cannot distinguish an empty sequence from an
-- empty object: an empty Lua table always becomes `{}`.  The durable recovery
-- manifest schema requires `spaces` to remain a JSON array even when an
-- intentionally empty owner has nothing to snapshot.  Correct only that
-- known field after encoding; every non-empty sequence is encoded normally.
local function write_recovery_manifest(candidate_id, manifest)
  if owner_recovery_manifest == nil then
    return false, 'native recovery manifest handle is unavailable'
  end
  local ok, encoded = pcall(wezterm.json_encode, manifest)
  if not ok then
    return false, 'cannot encode recovery manifest: ' .. tostring(encoded)
  end
  if type(manifest.spaces) == 'table' and next(manifest.spaces) == nil then
    local replacements
    encoded, replacements = encoded:gsub('"spaces":{}', '"spaces":[]', 1)
    if replacements ~= 1 then
      return false, 'cannot preserve empty recovery manifest spaces array'
    end
  end
  local written, write_error = pcall(function()
    return owner_recovery_manifest:write_candidate(candidate_id, encoded .. '\n')
  end)
  if not written then
    return false, 'cannot publish recovery manifest candidate: ' .. tostring(write_error)
  end
  return true
end

local function open_recovery_handles(epoch)
  if
    type(wezterm.mux.dmux_recovery_spool_open) ~= 'function'
    or type(wezterm.mux.dmux_recovery_manifest_open) ~= 'function'
  then
    return false, 'native recovery storage capabilities are unavailable'
  end
  local opened_spool, spool = pcall(wezterm.mux.dmux_recovery_spool_open, epoch)
  if not opened_spool then
    return false, 'cannot retain native recovery spool capability: ' .. tostring(spool)
  end
  local epoch_ok, retained_epoch = pcall(function()
    return spool:epoch()
  end)
  if not epoch_ok or retained_epoch ~= epoch then
    return false, 'native recovery spool capability returned a mismatched epoch'
  end
  local opened_manifest, manifest = pcall(wezterm.mux.dmux_recovery_manifest_open)
  if not opened_manifest then
    return false, 'cannot retain native recovery manifest capability: ' .. tostring(manifest)
  end
  owner_recovery_spool = spool
  owner_recovery_manifest = manifest
  return true
end

-- Complete in-process mux scan.  This never invokes wezterm cli; IDs and
-- titles come from the same mux object graph that the restore mutates.
local function native_snapshot(epoch)
  local windows = {}
  for _, window in ipairs(wezterm.mux.all_windows()) do
    local window_row = {
      window_id = tostring(window:window_id()),
      workspace = tostring(window:get_workspace()),
      tabs = {},
    }
    for _, tab in ipairs(window:tabs()) do
      local tab_row = { tab_id = tostring(tab:tab_id()), panes = {} }
      for _, pane in ipairs(tab:panes()) do
        local ok_title, title = pcall(function()
          return pane:get_title()
        end)
        local ok_domain, domain = pcall(function()
          return pane:get_domain_name()
        end)
        table.insert(tab_row.panes, {
          pane_id = tostring(pane:pane_id()),
          title = ok_title and tostring(title or '') or '',
          domain = ok_domain and tostring(domain or '') or nil,
        })
      end
      table.insert(window_row.tabs, tab_row)
    end
    table.insert(windows, window_row)
  end
  return { complete = true, server_epoch = epoch, windows = windows }
end

-- Canonical topology-only projection shared with Rust's
-- NativeTreePrecondition. Mutable pane titles are excluded; every native ID,
-- parent, workspace, and domain remains part of the compare-before-mutate
-- witness.
local function native_tree_precondition(epoch)
  local windows = {}
  for _, window in ipairs(wezterm.mux.all_windows()) do
    local window_row = {
      window_id = tostring(window:window_id()),
      workspace = tostring(window:get_workspace()),
      tabs = {},
    }
    for _, tab in ipairs(window:tabs()) do
      local tab_row = { tab_id = tostring(tab:tab_id()), panes = {} }
      for _, pane in ipairs(tab:panes()) do
        local ok_domain, domain = pcall(function()
          return pane:get_domain_name()
        end)
        table.insert(tab_row.panes, {
          pane_id = tostring(pane:pane_id()),
          domain = ok_domain and tostring(domain or '') or '',
        })
      end
      table.sort(tab_row.panes, function(left, right)
        if left.pane_id == right.pane_id then
          return left.domain < right.domain
        end
        return left.pane_id < right.pane_id
      end)
      table.insert(window_row.tabs, tab_row)
    end
    table.sort(window_row.tabs, function(left, right)
      return left.tab_id < right.tab_id
    end)
    table.insert(windows, window_row)
  end
  table.sort(windows, function(left, right)
    if left.window_id == right.window_id then
      return left.workspace < right.workspace
    end
    return left.window_id < right.window_id
  end)
  return { server_epoch = epoch, windows = windows }
end

local function exact_value_equal(left, right)
  if type(left) ~= type(right) then
    return false
  end
  if type(left) ~= 'table' then
    return left == right
  end
  for key, value in pairs(left) do
    if not exact_value_equal(value, right[key]) then
      return false
    end
  end
  for key, _ in pairs(right) do
    if left[key] == nil then
      return false
    end
  end
  return true
end

local function object_native_ids(object)
  if type(object) ~= 'table' or not object.window or not object.tab or not object.pane then
    return nil
  end
  local ok, ids = pcall(function()
    return {
      window_id = tostring(object.window:window_id()),
      tab_id = tostring(object.tab:tab_id()),
      pane_id = tostring(object.pane:pane_id()),
    }
  end)
  return ok and ids or nil
end

local function same_native_ids(left, right)
  return left
    and right
    and left.window_id == right.window_id
    and left.tab_id == right.tab_id
    and left.pane_id == right.pane_id
end

local function tree_contains_native(tree, target)
  if not target or type(tree) ~= 'table' or type(tree.windows) ~= 'table' then
    return false
  end
  for _, window in ipairs(tree.windows) do
    if window.window_id == target.window_id then
      for _, tab in ipairs(window.tabs or {}) do
        if tab.tab_id == target.tab_id then
          for _, pane in ipairs(tab.panes or {}) do
            if pane.pane_id == target.pane_id then
              return true
            end
          end
        end
      end
    end
  end
  return false
end

local function context_parent_id(node, context)
  if node.operation == 'space_root' then
    return nil
  end
  local parent
  if node.operation == 'group_root' then
    local first = '/spaces/' .. tostring(node.space_uid) .. '/groups/1/splits/L'
    parent = context.objects[first]
    local ids = object_native_ids(parent)
    return ids and ids.window_id or false
  end
  if node.operation == 'split' then
    parent = context.objects[node.parent_path]
    local ids = object_native_ids(parent)
    return ids and ids.pane_id or false
  end
  return false
end

local function titled_panes(title)
  local ids = {}
  for _, window in ipairs(wezterm.mux.all_windows()) do
    for _, tab in ipairs(window:tabs()) do
      for _, pane in ipairs(tab:panes()) do
        local ok, pane_title = pcall(function()
          return pane:get_title()
        end)
        if ok and pane_title == title then
          table.insert(ids, tostring(pane:pane_id()))
        end
      end
    end
  end
  table.sort(ids)
  return ids
end

local function recovery_fields(status, sentinel)
  local fields = {}
  for key, value in pairs(sentinel or {}) do
    fields[key] = value
  end
  if status and status.generation_uid then
    fields.recovery_generation = status.generation_uid
  end
  if status and status.manifest_id then
    fields.recovery_manifest_id = status.manifest_id
  end
  if status and status.error then
    fields.error = status.error
  end
  return fields
end

local function run_guarded_recovery(epoch, sentinel, control_action)
  local dmux_bin = os.getenv 'DMUX_BIN'
  local helper_bin = os.getenv 'DMUX_PANE_BOOTSTRAP'
  local backend_instance = os.getenv 'DMUX_BACKEND_INSTANCE'
  local start_token = owner_service_descriptor and owner_service_descriptor.start_token or nil
  if not dmux_bin or dmux_bin == '' or not helper_bin or helper_bin == '' then
    return nil, 'recovery requires DMUX_BIN and DMUX_PANE_BOOTSTRAP'
  end
  if not backend_instance or backend_instance == '' then
    return nil, 'recovery requires a durable backend instance'
  end
  if not start_token or start_token == '' then
    return nil, 'recovery requires a native-published service start token'
  end
  if owner_recovery_spool == nil or owner_recovery_manifest == nil then
    return nil, 'recovery requires retained native storage capabilities'
  end

  local ok_plugin, resurrect = pcall(wezterm.plugin.require, RESURRECT_URL)
  if not ok_plugin or type(resurrect) ~= 'table' or type(resurrect.dmux) ~= 'table' then
    return nil, 'resurrection fork has no dmux owner API: ' .. tostring(resurrect)
  end
  if type(resurrect.dmux.prepare_restore) ~= 'function' or type(resurrect.dmux.execute_restore_node) ~= 'function' then
    return nil, 'resurrection fork dmux API is incomplete'
  end
  owner_resurrect_dmux = resurrect.dmux

  local prior_status, prior_status_error = read_spool_json 'status'
  if prior_status_error then
    return nil, prior_status_error
  end
  local prior_coordinator_uid = prior_status and prior_status.coordinator_uid or nil
  local argv = {
    dmux_bin,
    '_recovery',
    'coordinate',
    '--backend-instance',
    backend_instance,
    '--server-epoch',
    epoch,
    '--runtime-dir',
    RUNTIME,
    '--server-pid',
    tostring(proc_pid()),
    '--server-start-token',
    start_token or '',
    '--helper-bin',
    helper_bin,
  }
  if control_action == 'resume' then
    table.insert(argv, '--resume-failed')
  elseif control_action == 'abort' then
    table.insert(argv, '--abort-failed')
  end
  local function spawn_coordinator()
    local spawned, spawn_error = pcall(wezterm.background_child_process, registry_only_argv(argv))
    if not spawned then
      return false, 'cannot start registry-only coordinator: ' .. tostring(spawn_error)
    end
    return true
  end
  local spawned, spawn_error = spawn_coordinator()
  if not spawned then
    return nil, spawn_error
  end

  local context = owner_recovery_context or { objects = {} }
  local coordinator_uid = nil
  local fencing_token = nil
  local bound_status_state = nil
  local bound_generation_uid = nil
  local last_sequence = 0
  local last_status_update = nil
  local last_activity = os.time()
  local restart_count = 0
  local deadline = os.time() + tonumber(os.getenv 'DMUX_RECOVERY_TIMEOUT_SECS' or '300')
  while os.time() <= deadline do
    local status, status_error = read_spool_json 'status'
    if status_error then
      return nil, status_error
    end
    if
      status
      and status.protocol_version == RECOVERY_PROTOCOL_VERSION
      and status.backend_instance_uid == backend_instance
      and status.server_epoch == epoch
    then
      local status_uid = type(status.coordinator_uid) == 'string' and status.coordinator_uid or nil
      local status_fence = type(status.fencing_token) == 'number' and status.fencing_token or nil
      local can_bind = status_uid and status_fence and (status.state == 'starting' or status.state == 'recovering')
      if can_bind then
        local replace
        if coordinator_uid == nil then
          -- The file may still contain the terminal/stale status that was
          -- present before this helper was spawned.  A coordinator must
          -- publish a fresh UID/fence handshake before Lua accepts commands.
          replace = status_uid ~= prior_coordinator_uid
        elseif status_uid == coordinator_uid then
          replace = status_fence >= fencing_token
        else
          -- Takeover is monotonic in the durable lease fencing token.  A
          -- dead helper's later file write can never re-bind the owner.
          replace = status_fence > fencing_token
        end
        if replace then
          if coordinator_uid ~= status_uid then
            coordinator_uid = status_uid
            last_sequence = 0
          end
          fencing_token = status_fence
          bound_status_state = status.state
          bound_generation_uid = status.generation_uid
        end
      end
      if
        coordinator_uid == nil
        and status_uid ~= prior_coordinator_uid
        and (status.state == 'failed' or status.state == 'aborted')
      then
        -- Validation may fail before the helper can acquire a lease and
        -- publish a numeric fence.  It has no native authority, but its fresh
        -- UID is still a safe terminal result for this launch attempt.
        coordinator_uid = status_uid
      end
      local status_is_current = status_uid == coordinator_uid and (status_fence == nil or status_fence == fencing_token)
      if status_is_current and status.updated_at ~= last_status_update then
        last_status_update = status.updated_at
        last_activity = os.time()
      end
      if status.state == 'ready' and status_is_current then
        return status
      elseif status.state == 'aborted' and status_is_current and control_action == 'abort' then
        return status
      elseif (status.state == 'failed' or status.state == 'aborted') and status_is_current then
        return nil, status.error or ('recovery ' .. tostring(status.state))
      elseif
        (status.state == 'starting' or status.state == 'recovering')
        and status_is_current
        and status_fence == fencing_token
      then
        local published, why = publish_descriptor('recovering', recovery_fields(status, sentinel))
        if not published then
          return nil, 'cannot refresh recovering descriptor: ' .. tostring(why)
        end
      end
    end

    ---@type DmuxRecoveryCommand|nil
    local command, command_error = read_spool_json 'command'
    if command_error then
      return nil, command_error
    end
    local command_valid = command
      and command.protocol_version == RECOVERY_PROTOCOL_VERSION
      and type(command.coordinator_uid) == 'string'
      and type(command.generation_uid) == 'string'
      and type(command.sequence) == 'number'
      and type(command.fencing_token) == 'number'
    --- Runtime access remains guarded by `command_valid`; this cast teaches
    --- the static checker the fields proven by that validation chain.
    ---@cast command DmuxRecoveryCommand
    local command_is_bound = command_valid
      and coordinator_uid ~= nil
      and fencing_token ~= nil
      and command.coordinator_uid == coordinator_uid
      and command.fencing_token == fencing_token
      and (bound_generation_uid == nil or command.generation_uid == bound_generation_uid)
      and (bound_status_state == 'starting' or bound_status_state == 'recovering')
    if command_is_bound and command.sequence > last_sequence then
      local expected_sequence = last_sequence + 1
      last_sequence = command.sequence
      last_activity = os.time()
      if owner_recovery_generation_uid ~= command.generation_uid then
        context = { objects = {} }
        owner_recovery_context = context
        owner_recovery_generation_uid = command.generation_uid
      end
      local response = {
        protocol_version = RECOVERY_PROTOCOL_VERSION,
        coordinator_uid = command.coordinator_uid,
        generation_uid = command.generation_uid,
        sequence = command.sequence,
        fencing_token = command.fencing_token,
        ok = false,
      }
      local ok_action, action_error = pcall(function()
        if command.sequence ~= expected_sequence then
          error(
            'non-monotonic recovery sequence '
              .. tostring(command.sequence)
              .. ', expected '
              .. tostring(expected_sequence)
          )
        end
        if command.action == 'inspect' or command.action == 'verify' then
          response.snapshot = native_snapshot(epoch)
        elseif command.action == 'prepare' then
          local prepared, why = resurrect.dmux.prepare_restore(command.nodes, context)
          if not prepared then
            error(why or 'Prepare rejected')
          end
          context = prepared
          owner_recovery_context = context
        elseif command.action == 'compare_and_restore_node' then
          local actual_tree = native_tree_precondition(epoch)
          if not exact_value_equal(actual_tree, command.expected_tree) then
            error 'native tree precondition changed'
          end
          local actual_parent = context_parent_id(command.node, context)
          if actual_parent == false or actual_parent ~= command.expected_parent then
            error 'native parent precondition changed'
          end
          local prepared_object = context.objects[command.node.manifest_node_path]
          if command.expected_existing then
            if command.create_if_absent then
              error 'existing recovery object cannot request creation'
            end
            local prepared_ids = object_native_ids(prepared_object)
            if
              not same_native_ids(prepared_ids, command.expected_existing)
              or not tree_contains_native(actual_tree, command.expected_existing)
            then
              error 'prepared recovery object differs from expected existing IDs'
            end
            response.created = command.expected_existing
          elseif command.create_if_absent then
            if prepared_object ~= nil then
              error 'fresh recovery create has an unexpected prepared object'
            end
            context.bootstrap_argv = command.bootstrap_argv
            -- No sleep, yield, file IO, or callback boundary is permitted
            -- between the raw-tree/parent comparison above and this native
            -- create. This is the §15.3 compare-and-mutate critical section.
            local restored, why = resurrect.dmux.execute_restore_node(command.node, context)
            if not restored then
              error(why or 'CompareAndRestoreNode rejected')
            end
            local expected_title = 'dmux-bootstrap:' .. command.request_uid
            local titled = {}
            for _ = 1, 250 do
              titled = titled_panes(expected_title)
              if #titled > 0 then
                break
              end
              wezterm.sleep_ms(20)
            end
            response.created = {
              window_id = tostring(restored.window:window_id()),
              tab_id = tostring(restored.tab:tab_id()),
              pane_id = tostring(restored.pane:pane_id()),
              titled_pane_ids = titled,
            }
          else
            -- A lost/removed provisional pane may leave only a stale Lua
            -- context object. The exact raw tree just proved it absent, so
            -- retire that non-native cache entry before Rust issues a new
            -- bootstrap request.
            context.objects[command.node.manifest_node_path] = nil
            response.existing_absent = true
          end
        elseif command.action == 'compare_and_remove_node' then
          local actual_tree = native_tree_precondition(epoch)
          if not exact_value_equal(actual_tree, command.expected_tree) then
            error 'native tree precondition changed'
          end
          if type(wezterm.mux.dmux_recovery_remove_node) ~= 'function' then
            error 'dmux recovery removal primitive is unavailable'
          end
          local pane_id = tonumber(command.pane_id)
          local tab_id = tonumber(command.tab_id)
          local window_id = tonumber(command.window_id)
          if not pane_id or not tab_id or not window_id then
            error 'compare_and_remove_node carries non-numeric native IDs'
          end
          -- As above, comparison and exact-ID mutation are one callback with
          -- no intervening yield.
          local removed = wezterm.mux.dmux_recovery_remove_node {
            kind = 'pane',
            native_id = pane_id,
            parent_tab_id = tab_id,
            parent_window_id = window_id,
          }
          if type(removed) ~= 'table' or (removed.status ~= 'removed' and removed.status ~= 'not_found') then
            error('compare_and_remove_node was not proven removed/absent: ' .. tostring(removed and removed.status))
          end
          context.objects[command.manifest_node_path] = nil
          response.removed = removed
        else
          error('unsupported recovery action ' .. tostring(command.action))
        end
      end)
      if ok_action then
        response.ok = true
      else
        response.error = tostring(action_error)
      end
      local written, why = write_spool_json('response', response)
      if not written then
        return nil, why
      end
    end
    -- `background_child_process` intentionally gives no child handle.  A
    -- hard coordinator crash therefore leaves the in-process owner alive
    -- but silent.  After more than one reply timeout with no status/command
    -- progress, start a takeover helper.  Its fresh coordinator UID lets
    -- sequence numbering restart while the prepared mux-object context is
    -- preserved for exact same-generation replay.
    if os.time() - last_activity >= 35 and restart_count < 3 then
      local restarted, why = spawn_coordinator()
      if not restarted then
        return nil, why
      end
      restart_count = restart_count + 1
      last_activity = os.time()
    end
    wezterm.sleep_ms(20)
  end
  return nil, 'recovery coordinator timed out'
end

local function schedule_recovery_control(epoch, sentinel)
  if not wezterm.time or type(wezterm.time.call_after) ~= 'function' then
    log 'recovery control timer unavailable; explicit resume requires a service restart'
    return
  end
  wezterm.time.call_after(1, function()
    local request, request_error = read_spool_json 'control'
    if request_error then
      log('cannot consume recovery control request: ' .. tostring(request_error))
      schedule_recovery_control(epoch, sentinel)
      return
    end
    if request then
      local removed, remove_error = remove_spool_message 'control'
      if not removed then
        log('cannot consume recovery control request: ' .. tostring(remove_error))
        schedule_recovery_control(epoch, sentinel)
        return
      end
      local backend_instance = os.getenv 'DMUX_BACKEND_INSTANCE'
      if
        request.protocol_version == RECOVERY_PROTOCOL_VERSION
        and (request.action == 'resume' or request.action == 'abort')
        and request.backend_instance_uid == backend_instance
        and request.server_epoch == epoch
      then
        local published, publish_error = publish_descriptor('recovering', sentinel)
        if not published then
          log('explicit recovery descriptor failed: ' .. tostring(publish_error))
          schedule_recovery_control(epoch, sentinel)
          return
        end
        local recovery, why = run_guarded_recovery(epoch, sentinel, request.action)
        if recovery then
          local ready, ready_error = publish_descriptor('ready', recovery_fields(recovery, sentinel))
          if not ready then
            log('explicit recovery ready descriptor failed: ' .. tostring(ready_error))
            schedule_recovery_control(epoch, sentinel)
            return
          end
          log(
            'explicit recovery '
              .. tostring(request.action)
              .. ' completed generation='
              .. tostring(recovery.generation_uid)
          )
          schedule_guarded_snapshot(epoch, 1)
          return
        end
        publish_descriptor('failed', recovery_fields({ error = tostring(why) }, sentinel))
        log('explicit recovery resume failed: ' .. tostring(why))
      else
        log 'ignored invalid recovery control request'
      end
    end
    schedule_recovery_control(epoch, sentinel)
  end)
end

-- Snapshot capture uses the same two-process split as restore.  The helper
-- first acquires the common backend lock and writes `<candidate>.plan`; only
-- then does this in-process owner inspect mux objects and answer with a
-- complete candidate.  The helper validates and atomically publishes it
-- before releasing the fence, so capture cannot overlap a mutation/recovery.
local function publish_guarded_snapshot(epoch)
  if not owner_resurrect_dmux or type(owner_resurrect_dmux.build_manifest) ~= 'function' then
    return false, 'resurrection fork dmux capture API is unavailable'
  end
  local dmux_bin = os.getenv 'DMUX_BIN'
  local backend_instance = os.getenv 'DMUX_BACKEND_INSTANCE'
  local start_token = owner_service_descriptor and owner_service_descriptor.start_token or nil
  if not dmux_bin or not backend_instance or not start_token or owner_recovery_manifest == nil then
    return false, 'snapshot environment is incomplete'
  end

  snapshot_serial = snapshot_serial + 1
  local stem = epoch .. '-' .. tostring(os.time()) .. '-' .. tostring(snapshot_serial)
  local candidate_id = '.capture-' .. stem
  local candidate_removed, candidate_remove_error = pcall(function()
    return owner_recovery_manifest:remove(candidate_id, 'candidate')
  end)
  if not candidate_removed then
    return false, 'cannot clear snapshot candidate slot: ' .. tostring(candidate_remove_error)
  end
  local plan_removed, plan_remove_error = pcall(function()
    return owner_recovery_manifest:remove(candidate_id, 'plan')
  end)
  if not plan_removed then
    return false, 'cannot clear snapshot plan slot: ' .. tostring(plan_remove_error)
  end

  local spawned, spawn_error = pcall(
    wezterm.background_child_process,
    registry_only_argv {
      dmux_bin,
      '_recovery',
      'snapshot-publish',
      '--backend-instance',
      backend_instance,
      '--candidate-id',
      candidate_id,
      '--server-epoch',
      epoch,
      '--runtime-dir',
      RUNTIME,
      '--server-pid',
      tostring(proc_pid()),
      '--server-start-token',
      start_token,
    }
  )
  if not spawned then
    return false, 'cannot start fenced snapshot helper: ' .. tostring(spawn_error)
  end

  local plan = nil
  for _ = 1, 250 do
    local plan_error
    plan, plan_error = read_manifest_json(candidate_id, 'plan')
    if plan_error then
      return false, plan_error
    end
    if plan then
      break
    end
    wezterm.sleep_ms(20)
  end
  if
    not plan
    or plan.protocol_version ~= RECOVERY_PROTOCOL_VERSION
    or plan.backend_instance_uid ~= backend_instance
    or plan.server_epoch ~= epoch
  then
    return false, 'snapshot helper did not publish an exact fenced capture plan'
  end
  local manifest, build_error = owner_resurrect_dmux.build_manifest {
    manifest_id = plan.manifest_id,
    backend_instance_uid = plan.backend_instance_uid,
    registry_revision = plan.registry_revision,
    generated_at = plan.generated_at,
    owner_domain = plan.owner_domain,
    spaces = plan.spaces,
  }
  if not manifest then
    return false, build_error or 'snapshot candidate capture failed'
  end
  local written, write_error = write_recovery_manifest(candidate_id, manifest)
  if not written then
    return false, write_error
  end

  for _ = 1, 1750 do
    local published, published_error = read_manifest_json(candidate_id, 'published')
    if published_error then
      return false, published_error
    end
    if published and published.manifest_id == plan.manifest_id then
      return true
    end
    -- The helper removes its plan on every terminal path.  Once absent, no
    -- valid publication can still be in flight.
    local pending, pending_error = read_manifest_json(candidate_id, 'plan')
    if pending_error then
      return false, pending_error
    end
    if not pending then
      break
    end
    wezterm.sleep_ms(20)
  end
  return false, 'snapshot helper did not publish the captured manifest'
end

schedule_guarded_snapshot = function(epoch, delay)
  if not wezterm.time or type(wezterm.time.call_after) ~= 'function' then
    log 'snapshot timer unavailable; guarded owner snapshots are disabled'
    return
  end
  wezterm.time.call_after(delay, function()
    local ok, why = publish_guarded_snapshot(epoch)
    if not ok then
      log('snapshot publication skipped: ' .. tostring(why))
    end
    schedule_guarded_snapshot(epoch, tonumber(os.getenv 'DMUX_SNAPSHOT_INTERVAL_SECS' or '300'))
  end)
end

local config = wezterm.config_builder()

-- This fixed service config is the sole descriptor publisher in both rollout
-- states. Recovery/snapshot execution remains separately gated by WEZ_FIRST;
-- arbitrary GUI configs receive no native service or recovery authority.
config.dmux_recovery_primitives = true
-- Recovery retains native directory capabilities and an in-memory prepared
-- mux-object graph for the lifetime of this server. A config reload would
-- replace that authority/context mid-generation; service changes take effect
-- only through an explicit service restart.
config.automatically_reload_config = false

config.unix_domains = {
  {
    name = 'dmux',
    socket_path = SOCK,
    -- Defense-in-depth only: this does NOT stop CLI auto-start (ADR 002).
    -- The load-bearing invariant is that every dmux CLI call carries
    -- --no-auto-start plus an explicit WEZTERM_UNIX_SOCKET.
    no_serve_automatically = true,
  },
}

-- Canary (permanent regression tripwire, ADR 002): default-shell suppression
-- works only because mux-startup leaves the mux non-empty. If WezTerm ever
-- spawns its unmanaged default program anyway, this marker command shows up
-- in `cli list` under workspace "default" instead of a usable shell.
config.default_prog = {
  '/bin/sh',
  '-c',
  'echo DMUX-CANARY-DEFAULT-PROG-MUST-NEVER-RUN; exec sleep 300',
}

-- The service runs the server FOREGROUND (--daemonize contends the shared
-- default pid-file lock, ADR 004). daemon_options stay pinned to the runtime
-- dir anyway so an accidental --daemonize can never collide with the live
-- GUI's ~/.local/share/wezterm.
config.daemon_options = {
  pid_file = RUNTIME .. '/wez-dmux.pid',
  stdout = RUNTIME .. '/wez-dmux.stdout.log',
  stderr = RUNTIME .. '/wez-dmux.stderr.log',
}

-- Bounded startup handler (ADR 002): in-process mux calls only. Never call
-- `wezterm cli` from here (guaranteed deadlock against our own starting
-- server), never io.popen, never recurse. Anything slow here delays serving.
wezterm.on('mux-startup', function()
  local epoch = os.getenv 'DMUX_SERVER_EPOCH' or 'epoch-env-missing'
  log('mux-startup BEGIN epoch=' .. epoch .. ' socket=' .. SOCK)
  if MISCONFIGURED then
    log('MISCONFIGURED: managed service bootstrap failed: ' .. tostring(SERVICE_BOOTSTRAP_ERROR))
  end
  local starting_descriptor, starting_error = publish_descriptor 'starting'
  if not starting_descriptor then
    -- Still create the reserved sentinel below so a publication failure can
    -- never expose WezTerm's unmanaged default program. Recovery and ready
    -- publication remain unavailable until the native witness succeeds.
    log('starting descriptor FAILED: ' .. tostring(starting_error))
  end

  local ok_pre, wins = pcall(wezterm.mux.all_windows)
  local pre = ok_pre and #wins or -1
  log('all_windows pre-spawn ok=' .. tostring(ok_pre) .. ' count=' .. tostring(pre))
  if pre ~= 0 then
    -- P5 records the anomaly; the P10 recovery algorithm owns acting on it.
    log('WARN mux not empty before sentinel spawn (count=' .. tostring(pre) .. ')')
  end

  -- Exactly one reserved sentinel window. It keeps an intentionally empty
  -- server non-empty (suppressing the default program), publishes the epoch
  -- through normal list fields, and is never a Space, Group, or Split.
  local dmux_bin = os.getenv 'DMUX_BIN'
  local sentinel_args
  local sentinel_fallback = false
  if dmux_bin and dmux_bin ~= '' then
    sentinel_args = { dmux_bin, '_mux-idle' }
  else
    -- Degraded but safe: keep the sentinel invariant alive without dmux.
    sentinel_args = { '/bin/sh', '-c', 'trap "" TERM; while :; do sleep 3600; done' }
    sentinel_fallback = true
    log 'WARN DMUX_BIN missing; sentinel running shell idle-loop fallback'
  end

  local ok_spawn, tab, pane, window = pcall(wezterm.mux.spawn_window, {
    workspace = 'dmux:system:' .. epoch,
    args = sentinel_args,
  })
  if not ok_spawn then
    log('sentinel spawn FAILED: ' .. tostring(tab))
    publish_descriptor('failed', { error = 'sentinel spawn failed: ' .. tostring(tab) })
    return
  end
  log(
    string.format(
      'sentinel spawned window_id=%s tab_id=%s pane_id=%s workspace=%s',
      tostring(window:window_id()),
      tostring(tab:tab_id()),
      tostring(pane:pane_id()),
      tostring(window:get_workspace())
    )
  )

  local sentinel = {
    sentinel_window_id = window:window_id(),
    sentinel_tab_id = tab:tab_id(),
    sentinel_pane_id = pane:pane_id(),
    sentinel_fallback = sentinel_fallback,
  }

  if not starting_descriptor then
    starting_descriptor, starting_error = publish_descriptor 'starting'
    if not starting_descriptor then
      log('starting descriptor retry FAILED: ' .. tostring(starting_error))
      return
    end
  end

  if not os.getenv 'DMUX_BACKEND_INSTANCE' or os.getenv 'DMUX_BACKEND_INSTANCE' == '' then
    local failed, failed_error = publish_descriptor(
      'failed',
      recovery_fields({
        error = 'managed backend identity is unavailable while DMUX_WEZ_FIRST is disabled',
      }, sentinel)
    )
    if not failed then
      log('missing backend identity failed descriptor FAILED: ' .. tostring(failed_error))
    end
    log 'mux-startup unavailable: no durable backend identity'
    return
  end

  if sentinel_fallback then
    local failed, failed_error =
      publish_descriptor('failed', recovery_fields({ error = 'managed sentinel requires dmux _mux-idle' }, sentinel))
    if not failed then
      log('fallback sentinel failed descriptor FAILED: ' .. tostring(failed_error))
    end
    log 'mux-startup unavailable: fallback sentinel cannot publish ready'
    return
  end

  if WEZ_FIRST then
    local storage_ready, storage_error = open_recovery_handles(epoch)
    if not storage_ready then
      log('native recovery storage FAILED: ' .. tostring(storage_error))
      publish_descriptor('failed', recovery_fields({ error = tostring(storage_error) }, sentinel))
      return
    end
    local recovering, recovering_error = publish_descriptor('recovering', sentinel)
    if not recovering then
      log('recovering descriptor FAILED: ' .. tostring(recovering_error))
      return
    end
    local recovery, recovery_error = run_guarded_recovery(epoch, sentinel, nil)
    if not recovery then
      log('recovery FAILED: ' .. tostring(recovery_error))
      publish_descriptor('failed', recovery_fields({ error = tostring(recovery_error) }, sentinel))
      schedule_recovery_control(epoch, sentinel)
      return
    end
    local ready, ready_error = publish_descriptor('ready', recovery_fields(recovery, sentinel))
    if not ready then
      log('ready descriptor FAILED: ' .. tostring(ready_error))
      return
    end
    log('mux-startup END recovery=' .. tostring(recovery.state) .. ' epoch=' .. epoch)
    schedule_guarded_snapshot(epoch, 1)
    return
  end

  local ready, ready_error = publish_descriptor('ready', sentinel)
  if not ready then
    log('ready descriptor FAILED: ' .. tostring(ready_error))
    return
  end
  log('mux-startup END epoch=' .. epoch)
end)

return config
