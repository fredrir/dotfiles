package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

-- Drives the owner mux config's `mux-startup` handler under a stubbed WezTerm.
-- The config is not a module: it returns WezTerm's config table, which must
-- carry no foreign keys, so the handler captured from `wezterm.on` is the only
-- seam for exercising the native-witness comparison and what follows it.
-- Every input the service wrapper would inject arrives through a replaced
-- os.getenv, never through the real environment.
local real_getenv = os.getenv
local env = {}
os.getenv = function(name) -- luacheck: ignore 122
  return env[name]
end

-- The owner mux config left this repository with dmux and is resolved where
-- dmux resolves it: DMUX_INTEGRATIONS_DIR for a checkout, otherwise the XDG
-- data directory install.sh writes to. Read through `real_getenv` — the stub
-- above answers only for the variables the service wrapper injects.
local function integrations_dir()
  local seam = real_getenv 'DMUX_INTEGRATIONS_DIR'
  if seam and seam ~= '' then
    return seam
  end
  local data = real_getenv 'XDG_DATA_HOME'
  if not data or data == '' or data:sub(1, 1) ~= '/' then
    data = (real_getenv 'HOME' or '') .. '/.local/share'
  end
  return data .. '/dmux/integrations'
end

local MUX_LUA = integrations_dir() .. '/wezterm-mux/dmux-mux.lua'

local SOCK = '/var/folders/xg/l7zk7hdd3f50h289ypshmp9m0000gn/T/dmux/wez-dmux.sock'
local RUNTIME = '/var/folders/xg/l7zk7hdd3f50h289ypshmp9m0000gn/T/dmux'
local PID = 54528
local EPOCH = '895ca35a-78ac-4ff7-ae9c-222a9aee3a81'
local NONCE = 'f856b455-b4cd-40cb-ae37-0d0332424485'
local INSTANCE = '0052fad8-58a2-4392-ae9d-3377a0c69fc2'
local DMUX_BIN = '/tmp/.local/bin/dmux'

-- mlua hands a Rust `None` to Lua as its JSON-null light userdata: type()
-- 'userdata', no metatable, `~= nil`. Plain Lua cannot construct a light
-- userdata, so the stand-in is a full userdata stripped of its metatable.
local NULL = assert(io.open('/dev/null', 'rb'))
debug.setmetatable(NULL, nil)
assert(type(NULL) == 'userdata' and getmetatable(NULL) == nil and NULL ~= nil)

local OPTIONAL = {
  'sentinel_window_id',
  'sentinel_tab_id',
  'sentinel_pane_id',
  'sentinel_fallback',
  'recovery_generation',
  'recovery_manifest_id',
  'error',
}

-- The shape `dmux_publish_service_descriptor` returns on the pinned fork:
-- request fields echoed, OS identities derived natively, an omitted
-- backend_instance_uid rendered as the null sentinel, plus `peer_pid`.
local function fork_descriptor(request, overrides)
  local descriptor = {
    descriptor_version = 1,
    state = request.state,
    epoch = request.epoch,
    pid = PID,
    socket = SOCK,
    socket_dev = 16777233,
    socket_ino = 14788383,
    start_token = 'macos:1787136165:343346',
    backend_instance_uid = NULL,
    boot_nonce = request.boot_nonce,
    boot_id = 'macos:1787129835:171112',
    written_by = 'mux-startup',
    written_at = '2026-08-19T10:42:45Z',
    peer_pid = PID,
  }
  if request.backend_instance_uid ~= nil then
    descriptor.backend_instance_uid = request.backend_instance_uid
  end
  for _, key in ipairs(OPTIONAL) do
    descriptor[key] = request[key]
  end
  for key, value in pairs(overrides or {}) do
    descriptor[key] = value
  end
  return descriptor, '{"descriptor_version":1}\n'
end

local logs, publishes, handlers, spawned = {}, {}, {}, nil
local publisher

local sentinel_tab = {
  tab_id = function()
    return 0
  end,
}
local sentinel_pane = {
  pane_id = function()
    return 0
  end,
}
local sentinel_window = {
  window_id = function()
    return 0
  end,
  get_workspace = function()
    return spawned and spawned.workspace
  end,
}

local fake_wezterm = {
  GLOBAL = {},
  config_builder = function()
    return {}
  end,
  log_info = function(message)
    table.insert(logs, message)
  end,
  log_error = function(message)
    table.insert(logs, message)
  end,
  on = function(name, callback)
    handlers[name] = callback
  end,
  procinfo = {
    pid = function()
      return PID
    end,
  },
  json_parse = function()
    error 'these scenarios never parse JSON'
  end,
  json_encode = function()
    error 'these scenarios never encode JSON'
  end,
  sleep_ms = function()
    error 'mux-startup must not sleep in these scenarios'
  end,
  time = {
    call_after = function()
      error 'mux-startup must not schedule work in these scenarios'
    end,
  },
  background_child_process = function()
    error 'mux-startup must not launch a helper in these scenarios'
  end,
  plugin = {
    require = function()
      error 'mux-startup must not load the resurrection fork in these scenarios'
    end,
  },
  mux = {
    dmux_service_bootstrap = function()
      return { api_version = 1, runtime_dir = RUNTIME, socket_path = SOCK }
    end,
    dmux_publish_service_descriptor = function(request)
      table.insert(publishes, request)
      return publisher(request)
    end,
    all_windows = function()
      return {}
    end,
    spawn_window = function(options)
      spawned = options
      return sentinel_tab, sentinel_pane, sentinel_window
    end,
    dmux_recovery_spool_open = function()
      error 'mux-startup must not open recovery storage in these scenarios'
    end,
    dmux_recovery_manifest_open = function()
      error 'mux-startup must not open recovery storage in these scenarios'
    end,
  },
}
package.loaded.wezterm = fake_wezterm

local function start(variables, scenario_publisher)
  env = {
    DMUX_SOCKET = SOCK,
    DMUX_RUNTIME_DIR = RUNTIME,
    DMUX_SERVER_EPOCH = EPOCH,
    DMUX_BOOT_NONCE = NONCE,
    DMUX_BIN = DMUX_BIN,
  }
  for name, value in pairs(variables or {}) do
    env[name] = value
  end
  logs, publishes, handlers, spawned = {}, {}, {}, nil
  publisher = scenario_publisher
  local readable = io.open(MUX_LUA, 'r')
  assert(
    readable,
    'the owner mux config is not installed at '
      .. MUX_LUA
      .. '; run install.sh in the dmux repo, or set DMUX_INTEGRATIONS_DIR to a checkout'
  )
  readable:close()
  local config = dofile(MUX_LUA)
  assert(config.unix_domains[1].socket_path == SOCK, 'bootstrap must agree with the fixed socket')
  assert(type(handlers['mux-startup']) == 'function', 'config must register mux-startup')
  handlers['mux-startup']()
  return publishes
end

local function logged(pattern, plain)
  for _, line in ipairs(logs) do
    if line:find(pattern, 1, plain) then
      return line
    end
  end
  return nil
end

local function states(requests)
  local out = {}
  for _, request in ipairs(requests) do
    table.insert(out, request.state)
  end
  return table.concat(out, ',')
end

-- 1. A flag-off start with the exact pinned-fork shape is accepted, and the
--    missing-instance path publishes `failed` carrying the sentinel witness.
local requests = start(nil, function(request)
  return fork_descriptor(request)
end)
assert(states(requests) == 'starting,failed', states(requests))
assert(requests[1].backend_instance_uid == nil)
assert(requests[2].error == 'managed backend identity is unavailable while DMUX_WEZ_FIRST is disabled')
assert(requests[2].sentinel_window_id == 0 and requests[2].sentinel_tab_id == 0 and requests[2].sentinel_pane_id == 0)
assert(requests[2].sentinel_fallback == false)
assert(spawned.workspace == 'dmux:system:' .. EPOCH)
assert(spawned.args[1] == DMUX_BIN and spawned.args[2] == '_mux-idle')
assert(not logged 'starting descriptor FAILED', 'the fork shape must be accepted')
assert(logged 'mux%-startup unavailable: no durable backend identity')

-- 2. An absent backend_instance_uid rendered as nil is equally absent.
requests = start(nil, function(request)
  local descriptor, raw = fork_descriptor(request)
  descriptor.backend_instance_uid = nil
  return descriptor, raw
end)
assert(states(requests) == 'starting,failed', states(requests))
assert(not logged 'starting descriptor FAILED')

-- 3. A foreign pid is refused on both attempts, the refusal names the field,
--    and the handler still tries to leave `failed` behind before returning.
requests = start(nil, function(request)
  return fork_descriptor(request, { pid = PID + 1 })
end)
assert(states(requests) == 'starting,starting,failed', states(requests))
assert(logged 'starting descriptor FAILED: native descriptor publisher returned a mismatched service witness %(pid%)$')
assert(logged 'starting descriptor retry FAILED: .*%(pid%)$')
assert(
  requests[3].error:find '^starting descriptor refused: native descriptor publisher returned a mismatched service witness %(pid%)$'
)
assert(requests[3].sentinel_window_id == 0 and requests[3].sentinel_fallback == false)
assert(logged 'failed descriptor after starting refusal FAILED: .*%(pid%)$')
assert(logged 'mux%-startup unavailable: starting descriptor refused')
assert(not logged 'no durable backend identity')
assert(not logged 'mux%-startup END')

-- 4. When only the `starting` witness is foreign, the `failed` publication is
--    accepted and logged as published.
requests = start(nil, function(request)
  if request.state == 'starting' then
    return fork_descriptor(request, { pid = PID + 1 })
  end
  return fork_descriptor(request)
end)
assert(states(requests) == 'starting,starting,failed', states(requests))
assert(logged 'failed descriptor published after starting refusal')
assert(not logged 'after starting refusal FAILED')

-- 5. A foreign socket is refused.
start(nil, function(request)
  return fork_descriptor(request, { socket = '/var/folders/xg/l7zk7hdd3f50h289ypshmp9m0000gn/T/other/wez-dmux.sock' })
end)
assert(logged 'starting descriptor FAILED: .*%(socket%)$')
assert(not logged 'no durable backend identity')

-- 6. A non-string or empty start_token is refused.
for _, token in ipairs { 1787136165, '' } do
  start(nil, function(request)
    return fork_descriptor(request, { start_token = token })
  end)
  assert(logged 'starting descriptor FAILED: .*%(start_token%)$', tostring(token))
  assert(not logged 'no durable backend identity')
end

-- 7. Every other identity field stays refused by name.
for field, value in pairs {
  descriptor_version = 2,
  state = 'ready',
  epoch = 'f0000000-0000-4000-8000-000000000000',
  boot_nonce = 'f0000000-0000-4000-8000-000000000000',
  written_by = 'wrapper',
  boot_id = '',
} do
  start(nil, function(request)
    return fork_descriptor(request, { [field] = value })
  end)
  assert(logged('starting descriptor FAILED: .*%(' .. field .. '%)$'), field)
end
start(nil, function(request)
  local descriptor = fork_descriptor(request)
  return descriptor, nil
end)
assert(logged 'starting descriptor FAILED: .*%(raw%)$')
start(nil, function(request)
  return fork_descriptor(request, { peer_pid = PID + 1 })
end)
assert(logged 'starting descriptor FAILED: native descriptor publisher returned a foreign socket peer$')

-- 8. A managed start (DMUX_BACKEND_INSTANCE set) passes the same comparison
--    and reaches ready.
requests = start({ DMUX_BACKEND_INSTANCE = INSTANCE }, function(request)
  return fork_descriptor(request)
end)
assert(states(requests) == 'starting,ready', states(requests))
assert(requests[1].backend_instance_uid == INSTANCE)
assert(requests[2].sentinel_window_id == 0 and requests[2].sentinel_fallback == false)
assert(logged('mux-startup END epoch=' .. EPOCH, true))

-- 9. A managed request whose uid comes back absent, or as another uid, is
--    refused: absence is accepted only when nothing was requested.
for _, returned in ipairs { NULL, 'ffffffff-ffff-4fff-8fff-ffffffffffff' } do
  start({ DMUX_BACKEND_INSTANCE = INSTANCE }, function(request)
    return fork_descriptor(request, { backend_instance_uid = returned })
  end)
  assert(logged 'starting descriptor FAILED: .*%(backend_instance_uid%)$')
  assert(not logged 'mux%-startup END')
end

-- 10. "Absent" is exactly nil or the native null. A userdata that carries a
--     metatable, or a uid nobody asked for, is refused.
for _, returned in ipairs { io.stdout, INSTANCE } do
  start(nil, function(request)
    return fork_descriptor(request, { backend_instance_uid = returned })
  end)
  assert(logged 'starting descriptor FAILED: .*%(backend_instance_uid%)$')
  assert(not logged 'no durable backend identity')
end

-- 11. A refusal that raises carries mlua's multi-line traceback. The `failed`
--     reason built from it must still be publishable: one line, no control
--     bytes.
requests = start(nil, function(request)
  if request.state == 'starting' then
    error('dmux_descriptor_socket_changed: fixed socket changed before publication\nstack traceback:\n\t[C]: in ?', 0)
  end
  return fork_descriptor(request)
end)
assert(states(requests) == 'starting,starting,failed', states(requests))
assert(requests[3].error:find '^starting descriptor refused: dmux_descriptor_socket_changed: fixed socket changed')
assert(not requests[3].error:find '%c', 'error text must carry no control bytes')
assert(#requests[3].error <= 1024)
assert(logged 'failed descriptor published after starting refusal')

-- 12. An oversized reason is cut at the native bound without splitting a
--     multi-byte sequence: 29 bytes of prefix plus 994 x's puts the first byte
--     of 'æ' at offset 1024, so the cut must fall before it.
requests = start(nil, function(request)
  if request.state == 'starting' then
    error(string.rep('x', 994) .. 'æøå', 0)
  end
  return fork_descriptor(request)
end)
assert(#requests[3].error == 1023, #requests[3].error)
assert(requests[3].error:byte(-1) == string.byte 'x')

os.getenv = real_getenv -- luacheck: ignore 122

io.stdout:write 'dmux mux-startup witness test: native null accepted, foreign witness refused, refusal publishes failed\n'
