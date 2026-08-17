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
      local descriptor = io.open(runtime .. '/wez-dmux.json', 'rb')
      if not descriptor then
        error 'dmux_bridge_descriptor_unavailable'
      end
      local body = descriptor:read '*a'
      descriptor:close()
      if body:match '"state"%s*:%s*"starting"' then
        error 'dmux_bridge_descriptor_not_ready: starting'
      end
      error 'dmux_bridge_descriptor_invalid'
    end,
    dmux_bridge_open = function()
      error 'preflight failure must not open a bridge lease'
    end,
  },
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
assert(not ok and tostring(err):match 'descriptor_unavailable')
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains == nil)
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domain_instances == nil)

local descriptor = assert(io.open(runtime .. '/wez-dmux.json', 'wb'))
assert(descriptor:write '{"state":"starting"}')
descriptor:close()
package.loaded['wez.domains'] = nil
ok, err = pcall(dofile, 'shared/wezterm/wezterm.lua')
assert(not ok and tostring(err):match 'descriptor_not_ready: starting')
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains == nil)
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domain_instances == nil)

io.stdout:write 'dmux top-level config test: missing/starting descriptor rejected\n'
