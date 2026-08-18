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
local proxy = 'env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=/run/user/1000/dmux/wez-dmux.sock '
  .. '/usr/bin/wezterm cli --prefer-mux --no-auto-start proxy'
local rows = {
  {
    name = 'dmux-b-usb',
    remote_address = '10.77.77.2',
    username = 'fredrir',
    remote_wezterm_path = '/usr/bin/wezterm',
    override_proxy_command = proxy,
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
-- Set to send a backend payload the GUI's own encoder would never produce.
local raw_response
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
    return true, raw_response or assert(json.encode(response)), ''
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
assert(domains[1].override_proxy_command == proxy, "a managed domain must dial the owner's exact socket")
assert(logs[1]:match 'dmux%-b%-ts unavailable')
local instances = assert(mux.managed_persistent_domain_instances())
local owners = assert(mux.managed_persistent_domain_owners())
assert(instances['dmux-b-usb'] == backend and instances['dmux-b-ts'] == nil)
assert(owners['dmux-b-usb'] == host and owners['dmux-b-ts'] == nil)
instances['dmux-b-usb'] = 'mutated-by-caller'
assert(mux.managed_persistent_domain_instances()['dmux-b-usb'] == backend, 'identity snapshot must be copied')
owners['dmux-b-usb'] = 'mutated-by-caller'
assert(mux.managed_persistent_domain_owners()['dmux-b-usb'] == host, 'owner snapshot must be copied')

response.result.domains[1].alternate_domains = { 'dmux-b-ts' }
assert(#mux.domains() == 0, 'alternate-domain identity mismatch must fail closed')
assert(next(mux.managed_persistent_domain_instances()) == nil, 'invalid reload must clear prior identities')
assert(next(mux.managed_persistent_domain_owners()) == nil, 'invalid reload must clear prior owners')

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
response.result.domains[1].override_proxy_command = nil
assert(#mux.domains() == 0, 'compatible row without a pinned proxy command must fail closed')

response.result.domains = clone(rows)
response.result.domains[2].override_proxy_command = proxy
assert(#mux.domains() == 0, 'an incompatible row must carry no proxy command')

response.result.domains = clone(rows)
response.result.domains[1].override_proxy_command = proxy:gsub(' %-%-no%-auto%-start', '')
assert(#mux.domains() == 0, 'a proxy command that may auto-start a server must fail closed')

response.result.domains = clone(rows)
response.result.domains[1].override_proxy_command =
  proxy:gsub('/run/user/1000/dmux/wez%-dmux%.sock', '/run/user/1000/wezterm.sock')
assert(#mux.domains() == 0, 'a proxy command naming another endpoint must fail closed')

response.result.domains = clone(rows)
response.result.domains[1].remote_wezterm_path = '/opt/homebrew/bin/wezterm'
assert(#mux.domains() == 0, 'a proxy command must name its own row executable')

response.result.domains = clone(rows)
response.result.domains[1].wez_socket = '/run/user/1000/dmux/wez-dmux.sock'
assert(#mux.domains() == 0, 'an unknown manifest key must fail closed')

response.result.domains = clone(rows)
response.result.domains[2].remote_wezterm_path = 'relative/wezterm'
assert(#mux.domains() == 0, 'optional incompatible executable path must still be absolute')
assert(#logs == 12)

response.result.domains = clone(rows)
response.result.domains[2].backend_instance_uid = 'dddddddd-dddd-4ddd-8ddd-dddddddddddd'
assert(#mux.domains() == 0, 'one owner cannot name multiple backend instances')

response.result.domains = clone(rows)
response.result.domains[2].host_uid = 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee'
assert(#mux.domains() == 0, 'one backend instance cannot alias multiple owners')

-- Integer arithmetic wraps, so math.abs(math.mininteger) is math.mininteger. An
-- absolute-value range guard admits the one integer no exact-JSON consumer can
-- carry, and priority holds no second bound of its own the way route_id does.
-- The GUI encoder refuses to write that literal, so the backend has to speak it.
response.result.domains = clone(rows)
local tampered_wire, substituted =
  assert(json.encode(response)):gsub('"priority":10', '"priority":-9223372036854775808', 1)
assert(substituted == 1, 'priority must be substituted into the manifest wire')
raw_response = tampered_wire
assert(#mux.domains() == 0, 'a wrapping negative priority must fail closed')
raw_response = nil

response.result.domains = clone(rows)
response.result.domains[1].alternate_domains = { 'dmux-b-ts' }
response.result.domains[2].compatible = true
response.result.domains[2].unavailable_reason = nil
response.result.domains[2].remote_wezterm_path = '/usr/bin/wezterm'
response.result.domains[2].override_proxy_command = proxy
response.result.domains[2].alternate_domains = { 'dmux-b-usb' }
domains = mux.domains()
instances = assert(mux.managed_persistent_domain_instances())
owners = assert(mux.managed_persistent_domain_owners())
assert(#domains == 2, 'compatible alternate routes must both remain configured')
assert(instances['dmux-b-usb'] == backend and instances['dmux-b-ts'] == backend)
assert(owners['dmux-b-usb'] == host and owners['dmux-b-ts'] == host)

io.stdout:write 'dmux remote manifest test: strict authority rows passed\n'
