package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

assert(os.getenv 'DMUX_WEZ_FIRST' == '1')

local json = require 'wez.dmux_bridge.json'

local function clone(value)
  if type(value) ~= 'table' then
    return value
  end
  local out = {}
  for key, item in pairs(value) do
    out[key] = clone(item)
  end
  return json.is_array(value) and json.array(out) or out
end

local host = '22222222-2222-4222-8222-222222222222'
local backend = '44444444-4444-4444-8444-444444444444'
local rows = {
  {
    name = 'dmux-b-usb',
    remote_address = '10.77.77.2',
    username = 'fredrir',
    remote_wezterm_path = '/usr/bin/wezterm',
    host_uid = host,
    backend_instance_uid = backend,
    route_id = 1,
    priority = 10,
    transport = 'wez-ssh',
    network_class = 'usb',
    alternate_domains = json.array(),
    compatible = true,
  },
  {
    name = 'dmux-b-ts',
    remote_address = '100.126.231.24',
    username = 'fredrir',
    host_uid = host,
    backend_instance_uid = backend,
    route_id = 2,
    priority = 20,
    transport = 'openssh',
    network_class = 'tailscale',
    alternate_domains = { 'dmux-b-usb' },
    compatible = false,
    unavailable_reason = 'remote build differs from the exact compatible build',
  },
}

local response = { schema_version = 1, ok = true, result = { domains = clone(rows) } }
local logs = {}
local act = setmetatable({}, {
  __index = function(_, name)
    return function(value)
      return { name = name, value = value }
    end
  end,
})
local fake_wezterm = {
  action = act,
  action_callback = function(callback)
    return callback
  end,
  home_dir = '/tmp',
  hostname = function()
    return 'macie'
  end,
  log_error = function(message)
    table.insert(logs, message)
  end,
  log_warn = function(message)
    table.insert(logs, message)
  end,
  run_child_process = function(argv)
    assert(argv[2] == '_gui' and argv[3] == 'domains')
    return true, assert(json.encode(response)), ''
  end,
  target_triple = 'aarch64-apple-darwin',
}
package.preload.wezterm = function()
  return fake_wezterm
end

local mux = require 'wez.remote.mux'
local domains = mux.domains()
assert(#domains == 1, 'only compatible authority rows may enter WezTerm domain config')
assert(domains[1].name == 'dmux-b-usb')
assert(domains[1].multiplexing == 'WezTerm' and domains[1].assume_shell == 'Posix')
assert(logs[1]:match 'dmux%-b%-ts unavailable')

response.result.domains[1].alternate_domains = { 'dmux-b-ts' }
assert(#mux.domains() == 0, 'alternate-domain identity mismatch must fail closed')

response.result.domains = clone(rows)
response.result.domains[1].unavailable_reason = 'must be omitted when compatible'
assert(#mux.domains() == 0, 'compatible row with unavailable_reason must fail closed')

response.result.domains = clone(rows)
response.result.domains[2].unavailable_reason = nil
assert(#mux.domains() == 0, 'incompatible row without reason must fail closed')

response.result.domains = clone(rows)
response.result.domains[1].remote_wezterm_path = nil
assert(#mux.domains() == 0, 'compatible row without an owner executable path must fail closed')

response.result.domains = clone(rows)
response.result.domains[2].remote_wezterm_path = 'relative/wezterm'
assert(#mux.domains() == 0, 'optional incompatible executable path must still be absolute')
assert(#logs == 6)

io.stdout:write 'dmux remote manifest test: strict authority rows passed\n'
