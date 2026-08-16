package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local runtime = assert(os.getenv 'DMUX_RUNTIME_DIR')
assert(os.execute(string.format('/bin/mkdir -p %q/bridge', runtime)))
local key = assert(io.open(runtime .. '/bridge/key', 'wb'))
assert(key:write '0123456789abcdef0123456789abcdef')
key:close()

local act = setmetatable({
  PopKeyTable = { name = 'PopKeyTable' },
  HideApplication = { name = 'HideApplication' },
  QuitApplication = { name = 'QuitApplication' },
}, {
  __index = function(_, name)
    return function(value)
      return { name = name, value = value }
    end
  end,
})

local events = {}

local fake_wezterm = {
  action = act,
  GLOBAL = {},
  home_dir = '/tmp',
  hostname = function()
    return 'dmux-test'
  end,
  target_triple = 'aarch64-apple-darwin',
  action_callback = function(callback)
    return { name = 'Callback', callback = callback }
  end,
  on = function(name, callback)
    events[name] = callback
  end,
  json_encode = function()
    return '{}'
  end,
  plugin = {
    require = function()
      error 'external workspace picker must not load in managed mode'
    end,
  },
}
package.preload.wezterm = function()
  return fake_wezterm
end

local bridge = require 'wez.dmux_bridge'
local config = {
  keys = { { key = 'q', mods = 'CMD', action = 'QuitApplication' } },
  key_tables = { resize_pane = { { key = 'h', action = 'AdjustPaneSize' } } },
  mouse_bindings = { { event = 'Up', action = 'CloseCurrentPane' } },
  launch_menu = { { label = 'unsafe', args = { 'sh' } } },
  unix_domains = { { name = 'dmux' } },
  ssh_domains = { { name = 'dmux-b-usb' } },
}
bridge.preflight(config)
assert(config.disable_default_key_bindings == true)
assert(config.disable_default_mouse_bindings == true)
assert(config.dmux_managed_gui == true)
assert(#config.keys == 0)
assert(next(config.key_tables) == nil)
assert(next(config.mouse_bindings) == nil)
assert(#config.launch_menu == 0)
assert(config.window_decorations == 'RESIZE')
assert(config.show_new_tab_button_in_tab_bar == false)
assert(config.show_close_tab_button_in_tabs == false)

-- Simulate later modules appending both superficially safe chords and
-- forbidden actions. Final apply rebuilds known presentation-neutral keys
-- instead of trusting any action merely because its chord is allowlisted.
config.keys = {
  { key = 'c', mods = 'CTRL', action = 'SpawnWindow' },
  { key = 'q', mods = 'CMD', action = 'QuitApplication' },
  { key = 'n', mods = 'CMD', action = 'SpawnWindow' },
  { key = 'A', mods = 'LEADER', action = 'AttachDomain' },
}
config.key_tables = {
  resize_pane = { { key = 'h', action = 'AdjustPaneSize' } },
  sync_mode = { { key = 'Escape', action = { name = 'SyncOff' } } },
}
config.mouse_bindings = { { event = 'Up', action = 'CloseCurrentPane' } }
config.launch_menu = { { label = 'unsafe', args = { 'ssh' } } }
bridge.apply(config)
assert(#fake_wezterm.GLOBAL.dmux_managed_persistent_domains == 2)
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains[1] == 'dmux')
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains[2] == 'dmux-b-usb')

local forbidden = {
  HideApplication = true,
  QuitApplication = true,
  SpawnWindow = true,
  AttachDomain = true,
  AdjustPaneSize = true,
}
local chords = {}
for _, binding in ipairs(config.keys) do
  assert(not forbidden[binding.action], 'forbidden action survived: ' .. tostring(binding.action))
  local chord = (binding.mods or 'NONE') .. ':' .. binding.key
  assert(not chords[chord], 'duplicate final chord: ' .. chord)
  chords[chord] = true
end
assert(chords['CMD:q'], 'safe Command+Q replacement missing')
assert(chords['CMD:n'], 'dmux new-Space Command+N missing')
assert(chords['LEADER:w'], 'dmux Space picker missing')
assert(chords['CTRL|SHIFT:w'], 'safe desktop window-close replacement missing')
assert(config.key_tables.resize_pane == nil)
assert(config.key_tables.sync_mode == nil)
assert(config.key_tables.dmux_resize_split)
assert(next(config.mouse_bindings) == nil)
assert(#config.launch_menu == 0)

local controller = require 'wez.dmux_bridge.controller'
local called
controller.run = function(window, pane, verb)
  called = { window = window, pane = pane, verb = verb }
  return {}
end
bridge.setup()
assert(type(events['dmux-managed-window-close-requested']) == 'function')
local event_window, event_pane = {}, {}
events['dmux-managed-window-close-requested'](event_window, event_pane)
assert(called and called.window == event_window and called.pane == event_pane and called.verb == 'safe-quit')
assert(type(events['gui-startup']) == 'function')
assert(type(events['gui-attached']) == 'function')

io.stdout:write(string.format('dmux bridge config test: %d sanitized keys\n', #config.keys))
