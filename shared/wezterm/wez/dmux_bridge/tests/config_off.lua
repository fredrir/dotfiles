package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local events = {}
local fake_wezterm = {
  GLOBAL = {},
  on = function(name)
    events[name] = true
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end

local bridge = require 'wez.dmux_bridge'
assert(not bridge.enabled(), 'flag-off test must run without DMUX_WEZ_FIRST=1')

local original_keys = { { key = 'q', mods = 'CMD', action = 'legacy-quit' } }
local original_tables = { legacy = { { key = 'x', action = 'legacy' } } }
local original_menu = { { label = 'legacy' } }
local config = {
  keys = original_keys,
  key_tables = original_tables,
  launch_menu = original_menu,
}

bridge.preflight(config)
bridge.apply(config)
bridge.setup()

assert(config.keys == original_keys)
assert(config.key_tables == original_tables)
assert(config.launch_menu == original_menu)
assert(config.dmux_managed_gui == nil)
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains == nil)
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domain_instances == nil)
assert(next(events) == nil)

io.stdout:write 'dmux bridge flag-off config test: unchanged\n'
