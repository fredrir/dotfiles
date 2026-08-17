local wezterm = require 'wezterm'
local act = wezterm.action

local M = {}

local DOMAIN = 'unix'
local managed_persistent_domain_instances
local MAX_JSON_INTEGER = 9007199254740991
local MAX_DESCRIPTOR_BYTES = 64 * 1024

local UUID = '^[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]$'

local function is_uuid(value)
  return type(value) == 'string' and value:match(UUID) ~= nil
end

local function bounded_string(value, maximum)
  return type(value) == 'string' and #value > 0 and #value <= maximum and not value:find '[%z\1-\31\127]'
end

local function positive_integer(value)
  return type(value) == 'number' and value % 1 == 0 and value > 0 and value <= MAX_JSON_INTEGER
end

local function nonnegative_integer(value)
  return type(value) == 'number' and value % 1 == 0 and value >= 0 and value <= MAX_JSON_INTEGER
end

local DESCRIPTOR_KEYS = {
  backend_instance_uid = true,
  boot_id = true,
  boot_nonce = true,
  descriptor_version = true,
  epoch = true,
  error = true,
  pid = true,
  recovery_generation = true,
  recovery_manifest_id = true,
  sentinel_fallback = true,
  sentinel_pane_id = true,
  sentinel_tab_id = true,
  sentinel_window_id = true,
  socket = true,
  socket_dev = true,
  socket_ino = true,
  start_token = true,
  state = true,
  written_at = true,
  written_by = true,
}

local function exact_descriptor_keys(descriptor)
  for key in pairs(descriptor) do
    if type(key) ~= 'string' or not DESCRIPTOR_KEYS[key] then
      return false
    end
  end
  return true
end

local function boot_id_platform(value)
  if type(value) ~= 'string' then
    return nil
  end
  local linux = value:match '^linux:(.+)$'
  if linux then
    return is_uuid(linux) and 'linux' or nil
  end
  local seconds, micros = value:match '^macos:(%d+):(%d+)$'
  local numeric_seconds, numeric_micros = tonumber(seconds), tonumber(micros)
  return positive_integer(numeric_seconds)
      and type(numeric_micros) == 'number'
      and numeric_micros % 1 == 0
      and numeric_micros >= 0
      and numeric_micros <= 999999
      and 'macos'
    or nil
end

local function process_start_platform(value)
  if type(value) ~= 'string' then
    return nil
  end
  local linux_ticks = value:match '^linux:(%d+)$'
  if linux_ticks then
    return positive_integer(tonumber(linux_ticks)) and 'linux' or nil
  end
  local seconds, micros = value:match '^macos:(%d+):(%d+)$'
  local numeric_seconds, numeric_micros = tonumber(seconds), tonumber(micros)
  return positive_integer(numeric_seconds)
      and type(numeric_micros) == 'number'
      and numeric_micros % 1 == 0
      and numeric_micros >= 0
      and numeric_micros <= 999999
      and 'macos'
    or nil
end

function M.managed_persistent_domain_instances()
  if type(managed_persistent_domain_instances) ~= 'table' then
    return nil
  end
  return { dmux = managed_persistent_domain_instances.dmux }
end

-- dmux wez-first flag (plan §15.1, P5). The chosen mechanism is the
-- DMUX_WEZ_FIRST=1 environment variable, checked at config evaluation: it is
-- per-process, so a scratch/test GUI can opt in while the user's live GUI --
-- which was launched without the variable -- provably re-evaluates to the
-- unchanged legacy config on every reload. (A flag file was rejected for P5
-- because the live GUI would see it on its next reload.)
--
-- With the flag on, the GUI becomes an attach-only client of the
-- service-owned managed mux: it declares the `dmux` unix client domain at
-- the exact socket published by the service's runtime descriptor and starts
-- with `connect dmux` instead of spawning a local mux window. The descriptor
-- is authoritative; if it cannot be read, config evaluation fails closed (a
-- guessed socket path could dial the wrong server, while falling through to
-- the default path would create an unmanaged local pane).
local function dmux_managed_descriptor()
  -- Maintained-fork boundary: resolve the fixed platform runtime internally,
  -- hold verified directory descriptors, and read only the current-UID 0600
  -- non-symlink descriptor with a hard size bound. Production config never
  -- accepts a caller-selected path or DMUX_RUNTIME_DIR/TMPDIR substitution.
  if type(wezterm.gui) ~= 'table' or type(wezterm.gui.dmux_read_mux_descriptor) ~= 'function' then
    return nil, 'maintained-fork descriptor reader is unavailable'
  end
  local read_ok, body = pcall(wezterm.gui.dmux_read_mux_descriptor, MAX_DESCRIPTOR_BYTES)
  if not read_ok then
    return nil, 'verified descriptor read failed: ' .. tostring(body)
  end
  if body == nil then
    return nil, 'descriptor is absent'
  end
  if type(body) ~= 'string' or #body == 0 or #body > MAX_DESCRIPTOR_BYTES then
    return nil, 'descriptor reader returned an invalid bounded document'
  end
  local ok, descriptor = pcall(wezterm.json_parse, body)
  if not ok or type(descriptor) ~= 'table' or not exact_descriptor_keys(descriptor) then
    return nil, 'descriptor is not a strict version-1 object'
  end
  if descriptor.state ~= 'ready' then
    return nil, 'descriptor is not ready: ' .. tostring(descriptor.state)
  end
  if descriptor.descriptor_version ~= 1 then
    return nil, 'descriptor has unsupported version: ' .. tostring(descriptor.descriptor_version)
  end
  if not is_uuid(descriptor.epoch) then
    return nil, 'descriptor has invalid epoch: ' .. tostring(descriptor.epoch)
  end
  if not is_uuid(descriptor.backend_instance_uid) then
    return nil, 'descriptor has invalid backend instance: ' .. tostring(descriptor.backend_instance_uid)
  end
  if not is_uuid(descriptor.boot_nonce) then
    return nil, 'descriptor has invalid boot nonce: ' .. tostring(descriptor.boot_nonce)
  end
  local boot_platform = boot_id_platform(descriptor.boot_id)
  if not boot_platform then
    return nil, 'descriptor has invalid current-boot witness: ' .. tostring(descriptor.boot_id)
  end
  if not bounded_string(descriptor.socket, 103) or not descriptor.socket:match '^/.+/dmux/wez%-dmux%.sock$' then
    return nil, 'descriptor has invalid fixed managed socket: ' .. tostring(descriptor.socket)
  end
  if not positive_integer(descriptor.pid) then
    return nil, 'descriptor has invalid server pid: ' .. tostring(descriptor.pid)
  end
  local start_platform = process_start_platform(descriptor.start_token)
  if not start_platform then
    return nil, 'descriptor has invalid verifiable process start token'
  end
  local expected_platform = wezterm.target_triple:find 'darwin' and 'macos' or 'linux'
  if boot_platform ~= expected_platform or start_platform ~= expected_platform then
    return nil, 'descriptor process/boot witnesses do not match this platform'
  end
  if not positive_integer(descriptor.socket_dev) or not positive_integer(descriptor.socket_ino) then
    return nil, 'descriptor has invalid socket device/inode witness'
  end
  if
    descriptor.written_by ~= 'mux-startup'
    or not bounded_string(descriptor.written_at, 32)
    or not descriptor.written_at:match '^%d%d%d%d%-%d%d%-%d%dT%d%d:%d%d:%d%dZ$'
  then
    return nil, 'descriptor has invalid writer witness'
  end
  if
    not nonnegative_integer(descriptor.sentinel_window_id)
    or not nonnegative_integer(descriptor.sentinel_tab_id)
    or not nonnegative_integer(descriptor.sentinel_pane_id)
    or descriptor.sentinel_fallback ~= false
  then
    return nil, 'descriptor has invalid managed sentinel witness'
  end
  if descriptor.recovery_generation ~= nil and not is_uuid(descriptor.recovery_generation) then
    return nil, 'descriptor has invalid recovery generation'
  end
  if descriptor.recovery_manifest_id ~= nil and not bounded_string(descriptor.recovery_manifest_id, 256) then
    return nil, 'descriptor has invalid recovery manifest id'
  end
  if descriptor.error ~= nil then
    return nil, 'descriptor reports failure: ' .. tostring(descriptor.error)
  end
  return descriptor
end

function M.apply(config)
  -- A failed/reloaded evaluation must not retain an identity from an older
  -- descriptor. The final bridge sanitizer consumes this private snapshot
  -- only after it has matched the exact final unix-domain configuration.
  managed_persistent_domain_instances = nil
  local wez_first = os.getenv 'DMUX_WEZ_FIRST' == '1'
  if wez_first then
    -- Attach-only mode declares no spawnable legacy client domain.  In
    -- particular, do not retain the `unix` domain plus LEADER+u/A shortcuts:
    -- either could bypass dmux identity and create an unmanaged pane while
    -- the managed mux is starting or recovering.
    config.unix_domains = {}
    local descriptor, why = dmux_managed_descriptor()
    if descriptor then
      table.insert(config.unix_domains, {
        name = 'dmux',
        socket_path = descriptor.socket,
        -- GUI-side defense in depth; the service manager is the only
        -- starter, and every dmux CLI call carries --no-auto-start.
        no_serve_automatically = true,
      })
      config.default_gui_startup_args = { 'connect', 'dmux' }
      managed_persistent_domain_instances = { dmux = descriptor.backend_instance_uid }
    else
      -- A flag-on GUI must never fall through to WezTerm's ordinary default
      -- startup, which would create an unmanaged local pane.  The top-level
      -- config treats this module as mandatory in managed mode; make the
      -- descriptor readiness failure explicit and fatal here.
      error('dmux wez-first: managed descriptor unavailable: ' .. tostring(why))
    end
  else
    -- Flag-off is the exact legacy domain/key behavior.
    config.unix_domains = { {
      name = DOMAIN,
    } }
  end

  local keys = config.keys or {}
  if not wez_first then
    table.insert(keys, {
      key = 'u',
      mods = 'LEADER',
      action = act.SpawnCommandInNewTab {
        domain = {
          DomainName = DOMAIN,
        },
      },
    })
    -- Detaching is LEADER+d (wez.remote), which works for any domain.
    table.insert(keys, {
      key = 'A',
      mods = 'LEADER',
      action = act.AttachDomain(DOMAIN),
    })
  end
  config.keys = keys
end

return M
