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
--   DMUX_RUNTIME_DIR       per-user runtime dir holding socket/descriptor/logs
--   DMUX_DESCRIPTOR        runtime descriptor JSON path
--   DMUX_SERVER_EPOCH      fresh UUID per server start
--   DMUX_START_TOKEN       service process start token (pid+timestamp nonce)
--   DMUX_BOOT_NONCE        fresh UUID per server start
--   DMUX_BACKEND_INSTANCE  backend-instance UID (placeholder, may be empty in P5)
--   DMUX_BIN               absolute dmux binary providing `_mux-idle`

local wezterm = require 'wezterm'

local SOCK = os.getenv 'DMUX_SOCKET'
local RUNTIME = os.getenv 'DMUX_RUNTIME_DIR' or '/tmp'
local DESCRIPTOR = os.getenv 'DMUX_DESCRIPTOR' or (RUNTIME .. '/wez-dmux.json')
local WEZ_FIRST = os.getenv 'DMUX_WEZ_FIRST' == '1'
local RECOVERY_PROTOCOL_VERSION = 1
local RESURRECT_URL = 'https://github.com/fredrir/resurrect.wezterm'
local owner_resurrect_dmux = nil
local owner_recovery_context = nil
local owner_recovery_generation_uid = nil
local snapshot_serial = 0
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
---@field manifest_node_path string|nil
---@field pane_id string|nil
---@field tab_id string|nil
---@field window_id string|nil

-- Fail SAFE, not loud: a Lua error here could make wezterm fall back to the
-- default config, which binds the live GUI's default socket under
-- ~/.local/share/wezterm. A misconfigured start must never do that, so bind a
-- quarantine path instead and record the problem.
local MISCONFIGURED = SOCK == nil
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
  local f = io.open(RUNTIME .. '/wez-dmux.log', 'a')
  if f then
    f:write(string.format('%s [pid=%d] %s\n', now(), proc_pid(), msg))
    f:close()
  end
end

local function json_string(value)
  return '"' .. tostring(value):gsub('\\', '\\\\'):gsub('"', '\\"') .. '"'
end

-- Atomic descriptor write (tmp + rename). Schema is descriptor_version 1;
-- socket_dev/socket_ino are published as null: per ADR 001, dmux verifies
-- socket identity itself (lstat dev/ino + LOCAL_PEERPID/SO_PEERCRED + the
-- sentinel-epoch-in-list handshake) and never trusts a recorded inode alone.
local function write_descriptor(state, extra)
  local doc = string.format(
    '{"descriptor_version":1,"state":%s,"epoch":%s,"pid":%d,"socket":%s,'
      .. '"socket_dev":null,"socket_ino":null,"start_token":%s,'
      .. '"backend_instance_uid":%s,"boot_nonce":%s,"written_by":"mux-startup",'
      .. '"written_at":%s%s}\n',
    json_string(state),
    json_string(os.getenv 'DMUX_SERVER_EPOCH' or 'epoch-env-missing'),
    proc_pid(),
    json_string(SOCK),
    json_string(os.getenv 'DMUX_START_TOKEN' or ''),
    json_string(os.getenv 'DMUX_BACKEND_INSTANCE' or ''),
    json_string(os.getenv 'DMUX_BOOT_NONCE' or ''),
    json_string(now()),
    extra or ''
  )
  local tmp = DESCRIPTOR .. '.tmp'
  local f = io.open(tmp, 'w')
  if f then
    f:write(doc)
    f:close()
    os.rename(tmp, DESCRIPTOR)
  else
    log('descriptor write failed: ' .. tmp)
  end
end

local function read_json(path)
  local f = io.open(path, 'r')
  if not f then
    return nil
  end
  local body = f:read '*a'
  f:close()
  local ok, value = pcall(wezterm.json_parse, body)
  if not ok or type(value) ~= 'table' then
    return nil
  end
  return value
end

local function write_json(path, value)
  local tmp = path .. '.tmp'
  local f = io.open(tmp, 'w')
  if not f then
    return false, 'cannot open ' .. tmp
  end
  local ok, encoded = pcall(wezterm.json_encode, value)
  if not ok then
    f:close()
    os.remove(tmp)
    return false, 'cannot encode response: ' .. tostring(encoded)
  end
  f:write(encoded, '\n')
  f:flush()
  f:close()
  local renamed, why = os.rename(tmp, path)
  if not renamed then
    os.remove(tmp)
    return false, 'cannot publish ' .. path .. ': ' .. tostring(why)
  end
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

local function recovery_extra(status, sentinel)
  local extra = sentinel or ''
  if status and status.generation_uid then
    extra = extra .. ',"recovery_generation":' .. json_string(status.generation_uid)
  end
  if status and status.manifest_id then
    extra = extra .. ',"recovery_manifest_id":' .. json_string(status.manifest_id)
  end
  if status and status.error then
    extra = extra .. ',"error":' .. json_string(status.error)
  end
  return extra
end

local function run_guarded_recovery(epoch, sentinel, control_action)
  local dmux_bin = os.getenv 'DMUX_BIN'
  local helper_bin = os.getenv 'DMUX_PANE_BOOTSTRAP'
  local backend_instance = os.getenv 'DMUX_BACKEND_INSTANCE'
  local manifest_dir = os.getenv 'DMUX_RECOVERY_MANIFEST_DIR'
  local start_token = os.getenv 'DMUX_START_TOKEN'
  if not dmux_bin or dmux_bin == '' or not helper_bin or helper_bin == '' then
    return nil, 'recovery requires DMUX_BIN and DMUX_PANE_BOOTSTRAP'
  end
  if not backend_instance or backend_instance == '' or not manifest_dir or manifest_dir == '' then
    return nil, 'recovery requires backend instance and manifest directory'
  end

  local ok_plugin, resurrect = pcall(wezterm.plugin.require, RESURRECT_URL)
  if not ok_plugin or type(resurrect) ~= 'table' or type(resurrect.dmux) ~= 'table' then
    return nil, 'resurrection fork has no dmux owner API: ' .. tostring(resurrect)
  end
  if type(resurrect.dmux.prepare_restore) ~= 'function' or type(resurrect.dmux.execute_restore_node) ~= 'function' then
    return nil, 'resurrection fork dmux API is incomplete'
  end
  owner_resurrect_dmux = resurrect.dmux

  local spool = RUNTIME .. '/recovery/' .. epoch
  local command_path = spool .. '/command.json'
  local response_path = spool .. '/response.json'
  local status_path = spool .. '/status.json'
  local prior_status = read_json(status_path)
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
    '--manifest-dir',
    manifest_dir,
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
    local spawned, spawn_error = pcall(wezterm.background_child_process, argv)
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
    local status = read_json(status_path)
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
        write_descriptor('recovering', recovery_extra(status, sentinel))
      end
    end

    ---@type DmuxRecoveryCommand|nil
    local command = read_json(command_path)
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
        elseif command.action == 'restore_node' then
          context.bootstrap_argv = command.bootstrap_argv
          local restored, why = resurrect.dmux.execute_restore_node(command.node, context)
          if not restored then
            error(why or 'RestoreNode rejected')
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
        elseif command.action == 'remove_node' then
          if type(wezterm.mux.dmux_recovery_remove_node) ~= 'function' then
            error 'dmux recovery removal primitive is unavailable'
          end
          local pane_id = tonumber(command.pane_id)
          local tab_id = tonumber(command.tab_id)
          local window_id = tonumber(command.window_id)
          if not pane_id or not tab_id or not window_id then
            error 'remove_node carries non-numeric native IDs'
          end
          local removed = wezterm.mux.dmux_recovery_remove_node {
            kind = 'pane',
            native_id = pane_id,
            parent_tab_id = tab_id,
            parent_window_id = window_id,
          }
          if type(removed) ~= 'table' or (removed.status ~= 'removed' and removed.status ~= 'not_found') then
            error('remove_node was not proven removed/absent: ' .. tostring(removed and removed.status))
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
      local written, why = write_json(response_path, response)
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
  local control_path = RUNTIME .. '/recovery/' .. epoch .. '/control.json'
  wezterm.time.call_after(1, function()
    local request = read_json(control_path)
    if request then
      os.remove(control_path)
      local backend_instance = os.getenv 'DMUX_BACKEND_INSTANCE'
      if
        request.protocol_version == RECOVERY_PROTOCOL_VERSION
        and (request.action == 'resume' or request.action == 'abort')
        and request.backend_instance_uid == backend_instance
        and request.server_epoch == epoch
      then
        write_descriptor('recovering', sentinel)
        local recovery, why = run_guarded_recovery(epoch, sentinel, request.action)
        if recovery then
          write_descriptor('ready', recovery_extra(recovery, sentinel))
          log(
            'explicit recovery '
              .. tostring(request.action)
              .. ' completed generation='
              .. tostring(recovery.generation_uid)
          )
          schedule_guarded_snapshot(epoch, 1)
          return
        end
        write_descriptor('failed', sentinel .. ',"error":' .. json_string(tostring(why)))
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
  local manifest_dir = os.getenv 'DMUX_RECOVERY_MANIFEST_DIR'
  if not dmux_bin or not backend_instance or not manifest_dir then
    return false, 'snapshot environment is incomplete'
  end

  snapshot_serial = snapshot_serial + 1
  local stem = epoch .. '-' .. tostring(os.time()) .. '-' .. tostring(snapshot_serial)
  local candidate = manifest_dir .. '/.capture-' .. stem
  local plan_path = candidate .. '.plan'
  local destination = manifest_dir .. '/manifest-' .. stem .. '.json'
  os.remove(candidate)
  os.remove(plan_path)

  local spawned, spawn_error = pcall(wezterm.background_child_process, {
    dmux_bin,
    '_recovery',
    'snapshot-publish',
    '--backend-instance',
    backend_instance,
    '--candidate',
    candidate,
    '--destination',
    destination,
  })
  if not spawned then
    return false, 'cannot start fenced snapshot helper: ' .. tostring(spawn_error)
  end

  local plan = nil
  for _ = 1, 250 do
    plan = read_json(plan_path)
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
  local written, write_error = write_json(candidate, manifest)
  if not written then
    return false, write_error
  end

  for _ = 1, 1750 do
    local published = read_json(destination)
    if published and published.manifest_id == plan.manifest_id then
      return true
    end
    -- The helper removes its plan on every terminal path.  Once absent, no
    -- valid publication can still be in flight.
    if not read_json(plan_path) then
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

-- The minimal fork primitive is capability-gated and exposed only inside
-- this service-owned mux.  GUI configs and the flag-off stock build never
-- receive native recovery deletion authority.
if WEZ_FIRST then
  config.dmux_recovery_primitives = true
end

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
    log 'MISCONFIGURED: DMUX_SOCKET missing; serving quarantine socket only'
  end
  write_descriptor 'starting'

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
    write_descriptor('failed', ',"error":' .. json_string('sentinel spawn failed: ' .. tostring(tab)))
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

  local sentinel_extra = string.format(
    ',"sentinel_window_id":%d,"sentinel_tab_id":%d,"sentinel_pane_id":%d,"sentinel_fallback":%s',
    window:window_id(),
    tab:tab_id(),
    pane:pane_id(),
    tostring(sentinel_fallback)
  )

  if WEZ_FIRST then
    write_descriptor('recovering', sentinel_extra)
    local recovery, recovery_error = run_guarded_recovery(epoch, sentinel_extra, nil)
    if not recovery then
      log('recovery FAILED: ' .. tostring(recovery_error))
      write_descriptor('failed', sentinel_extra .. ',"error":' .. json_string(tostring(recovery_error)))
      schedule_recovery_control(epoch, sentinel_extra)
      return
    end
    write_descriptor('ready', recovery_extra(recovery, sentinel_extra))
    log('mux-startup END recovery=' .. tostring(recovery.state) .. ' epoch=' .. epoch)
    schedule_guarded_snapshot(epoch, 1)
    return
  end

  write_descriptor('ready', sentinel_extra)
  log('mux-startup END epoch=' .. epoch)
end)

return config
