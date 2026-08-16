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

local act = setmetatable({}, {
  __index = function(_, name)
    return function(value)
      return { name = name, value = value }
    end
  end,
})
local fake_wezterm = {
  GLOBAL = {},
  action = act,
  config_builder = function()
    return {}
  end,
  target_triple = 'aarch64-apple-darwin',
  json_parse = function()
    return { state = 'starting' }
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end
for _, name in ipairs {
  'wez.appearance',
  'wez.perf',
  'wez.keys',
  'wez.remote',
  'wez.integrations',
  'wez.plugins',
} do
  package.preload[name] = function()
    return { apply = function() end }
  end
end

local ok, err = pcall(dofile, 'shared/wezterm/wezterm.lua')
assert(not ok and tostring(err):match 'managed descriptor unavailable')
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains == nil)

local descriptor = assert(io.open(runtime .. '/wez-dmux.json', 'wb'))
assert(descriptor:write '{"state":"starting"}')
descriptor:close()
package.loaded['wez.domains'] = nil
ok, err = pcall(dofile, 'shared/wezterm/wezterm.lua')
assert(not ok and tostring(err):match 'descriptor is not ready: starting')
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains == nil)

io.stdout:write 'dmux top-level config test: missing/starting descriptor rejected\n'
