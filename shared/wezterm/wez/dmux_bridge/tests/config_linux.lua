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

io.stdout:write(string.format('dmux bridge Linux config test: %d sanitized keys\n', #config.keys))
