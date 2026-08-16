assert(os.getenv 'DMUX_WEZ_FIRST' ~= '1')

local config = { untouched = true }
local order = {}
local fake_wezterm = {
  config_builder = function()
    return config
  end,
  log_error = function(message)
    error(message)
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end
package.preload['wez.dmux_bridge'] = function()
  error 'flag-off top level must not load dmux bridge'
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
for _, name in ipairs(module_names) do
  package.preload[name] = function()
    return {
      apply = function()
        table.insert(order, 'apply:' .. name)
      end,
      setup = function()
        table.insert(order, 'setup:' .. name)
      end,
    }
  end
end

local result = dofile 'shared/wezterm/wezterm.lua'
assert(result == config and result.untouched == true)
assert(result.dmux_managed_gui == nil)
for index, name in ipairs(module_names) do
  assert(order[index * 2 - 1] == 'apply:' .. name)
  assert(order[index * 2] == 'setup:' .. name)
end

io.stdout:write 'dmux top-level flag-off test: legacy order unchanged\n'
