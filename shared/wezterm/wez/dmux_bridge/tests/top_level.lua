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
  GLOBAL = {},
  action = act,
  action_callback = function(callback)
    return { name = 'Callback', callback = callback }
  end,
  config_builder = function()
    return {}
  end,
  home_dir = '/tmp',
  hostname = function()
    return 'dmux-test'
  end,
  json_encode = function()
    return '{}'
  end,
  log_error = function(message)
    error(message)
  end,
  on = function(name, callback)
    events[name] = callback
  end,
  target_triple = 'aarch64-apple-darwin',
}
package.preload.wezterm = function()
  return fake_wezterm
end
package.preload['wez.platform'] = function()
  return { is_mac = true }
end
package.preload['wez.plugins.workspace_picker'] = function()
  return {
    action = function()
      return { name = 'DmuxPicker' }
    end,
  }
end

local module_names = {
  'wez.appearance',
  'wez.perf',
  'wez.keys',
  'wez.domains',
  'wez.remote',
  'wez.integrations',
  'wez.plugins',
}
local order = {}
for _, name in ipairs(module_names) do
  package.preload[name] = function()
    return {
      apply = function(config)
        assert(config.dmux_managed_gui == true, name .. ' ran before managed preflight')
        assert(config.disable_default_key_bindings == true)
        table.insert(order, 'apply:' .. name)
        table.insert(config.keys, { key = 'q', mods = 'CMD', action = 'QuitApplication' })
        table.insert(config.keys, { key = 'n', mods = 'CMD', action = 'SpawnWindow' })
        config.key_tables.unsafe = { { key = 'x', action = 'CloseCurrentPane' } }
        config.mouse_bindings = { { event = 'Up', action = 'CloseCurrentPane' } }
        table.insert(config.launch_menu, { label = 'unsafe native spawn' })
        if name == 'wez.domains' then
          config.unix_domains = { { name = 'dmux' } }
        elseif name == 'wez.remote' then
          config.ssh_domains = { { name = 'dmux-b-usb' } }
        end
      end,
      setup = function()
        table.insert(order, 'setup:' .. name)
      end,
    }
  end
end

local config = dofile 'shared/wezterm/wezterm.lua'
assert(config.dmux_managed_gui == true)
assert(config.disable_default_key_bindings == true)
assert(config.disable_default_mouse_bindings == true)
assert(config.window_decorations == 'RESIZE')
assert(#config.launch_menu == 0)
assert(config.key_tables.unsafe == nil)
assert(config.key_tables.dmux_resize_split)
assert(next(config.mouse_bindings) == nil)
assert(config.show_new_tab_button_in_tab_bar == false)
assert(config.show_close_tab_button_in_tabs == false)
assert(#fake_wezterm.GLOBAL.dmux_managed_persistent_domains == 2)
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains[1] == 'dmux')
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains[2] == 'dmux-b-usb')
for _, binding in ipairs(config.keys) do
  assert(binding.action ~= 'QuitApplication' and binding.action ~= 'SpawnWindow')
end
for index, name in ipairs(module_names) do
  assert(order[index * 2 - 1] == 'apply:' .. name)
  assert(order[index * 2] == 'setup:' .. name)
end
assert(type(events['dmux-managed-window-close-requested']) == 'function')
assert(type(events['gui-startup']) == 'function')
assert(type(events['gui-attached']) == 'function')

io.stdout:write 'dmux top-level config test: preflight/modules/final sanitizer ordered\n'
