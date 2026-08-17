package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

assert(os.getenv 'DMUX_WEZ_FIRST' == '1')
local runtime = assert(os.getenv 'DMUX_RUNTIME_DIR')
assert(os.execute(string.format('/bin/mkdir -p %q/bridge', runtime)))
local key = assert(io.open(runtime .. '/bridge/key', 'wb'))
assert(key:write '0123456789abcdef0123456789abcdef')
key:close()

local act = setmetatable({ PopKeyTable = { name = 'PopKeyTable' } }, {
  __index = function(_, name)
    return function(value)
      return { name = name, value = value }
    end
  end,
})
local fake_wezterm = {
  GLOBAL = {},
  action = act,
  action_callback = function(callback)
    return { name = 'Callback', callback = callback }
  end,
  home_dir = '/tmp',
  target_triple = 'x86_64-unknown-linux-gnu',
  gui = {
    dmux_bridge_capabilities = function()
      return {
        version = 1,
        descriptor_backed_spool = true,
        exclusive_instance_lease = true,
        launcher_witness = true,
        checked_preflight = true,
        capability_bound_lifecycle_completion = true,
        zero_window_lifecycle = true,
        verified_mux_descriptor = true,
      }
    end,
    dmux_bridge_preflight = function()
      return {
        version = 1,
        key_bytes = 32,
        runtime_verified = true,
        verified_mux_descriptor = true,
        launcher_witness_present = false,
      }
    end,
    dmux_bridge_open = function()
      error 'config test must not acquire a bridge lease'
    end,
  },
}
package.preload.wezterm = function()
  return fake_wezterm
end
package.preload['wez.platform'] = function()
  return { is_mac = false }
end
package.preload['wez.plugins.workspace_picker'] = function()
  return {
    action = function()
      return { name = 'DmuxPicker' }
    end,
  }
end
local backend_instance = '44444444-4444-4444-8444-444444444444'
package.preload['wez.domains'] = function()
  return {
    managed_persistent_domain_instances = function()
      return { dmux = backend_instance }
    end,
  }
end
package.preload['wez.remote.mux'] = function()
  return {
    managed_persistent_domain_instances = function()
      return {}
    end,
    managed_persistent_domain_owners = function()
      return {}
    end,
  }
end

local config = {
  keys = {},
  key_tables = {},
  mouse_bindings = {},
  launch_menu = {},
  unix_domains = { { name = 'dmux' } },
  ssh_domains = {},
}
require('wez.dmux_bridge').apply(config)

local chords = {}
for _, binding in ipairs(config.keys) do
  local chord = (binding.mods or 'NONE') .. ':' .. binding.key
  assert(not chords[chord], 'duplicate Linux managed chord: ' .. chord)
  assert(not (binding.mods or ''):find 'CMD', 'Linux managed config exposed a Command binding')
  chords[chord] = true
end
assert(chords['ALT:F4'])
assert(chords['CTRL|SHIFT:w'])
assert(chords['CTRL:w'])
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains[1] == 'dmux')
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domain_instances.dmux == backend_instance)

io.stdout:write(string.format('dmux bridge Linux config test: %d sanitized keys\n', #config.keys))
