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

local config = wezterm.config_builder()

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

  write_descriptor(
    'ready',
    string.format(
      ',"sentinel_window_id":%d,"sentinel_tab_id":%d,"sentinel_pane_id":%d,"sentinel_fallback":%s',
      window:window_id(),
      tab:tab_id(),
      pane:pane_id(),
      tostring(sentinel_fallback)
    )
  )
  log('mux-startup END epoch=' .. epoch)
end)

return config
