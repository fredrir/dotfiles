package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

assert(os.getenv 'DMUX_WEZ_FIRST' == '1')

local json = require 'wez.dmux_bridge.json'
local descriptor_body
local act = setmetatable({}, {
  __index = function(_, name)
    return function(value)
      return { name = name, value = value }
    end
  end,
})
local fake_wezterm = {
  action = act,
  gui = {
    dmux_read_mux_descriptor = function(maximum)
      assert(maximum == 64 * 1024)
      return descriptor_body
    end,
  },
  json_parse = function(body)
    local value, err = json.decode(body)
    if not value then
      error(err)
    end
    return value
  end,
  target_triple = 'aarch64-apple-darwin',
}
package.preload.wezterm = function()
  return fake_wezterm
end
local descriptor_reader = fake_wezterm.gui.dmux_read_mux_descriptor

local epoch = '55555555-5555-4555-8555-555555555555'
local backend_instance = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
local boot_nonce = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
local boot_id = 'macos:1700000000:123456'
local function write_descriptor(instance, extra, socket, start_token)
  descriptor_body = string.format(
    '{"descriptor_version":1,"state":"ready","epoch":"%s","pid":4242,'
      .. '"socket":"%s","socket_dev":42,"socket_ino":84,"start_token":"%s",'
      .. '"backend_instance_uid":"%s","boot_nonce":"%s","boot_id":"%s",'
      .. '"written_by":"mux-startup","written_at":"2026-08-17T12:34:56Z",'
      .. '"sentinel_window_id":0,"sentinel_tab_id":0,"sentinel_pane_id":0,'
      .. '"sentinel_fallback":false%s}',
    epoch,
    socket or '/private/tmp/dmux-test/dmux/wez-dmux.sock',
    start_token or 'macos:1700000100:654321',
    instance,
    boot_nonce,
    boot_id,
    extra or ''
  )
end

write_descriptor(backend_instance)
local domains = require 'wez.domains'
local config = { keys = {} }
domains.apply(config)
assert(#config.unix_domains == 1 and config.unix_domains[1].name == 'dmux')
assert(config.default_gui_startup_args[1] == 'connect' and config.default_gui_startup_args[2] == 'dmux')
local instances = assert(domains.managed_persistent_domain_instances())
assert(instances.dmux == backend_instance)
instances.dmux = 'mutated-by-caller'
assert(domains.managed_persistent_domain_instances().dmux == backend_instance, 'identity snapshot must be copied')

write_descriptor(backend_instance)
descriptor_body = descriptor_body:gsub('"sentinel_pane_id":0', '"sentinel_pane_id":-1')
local ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'invalid managed sentinel witness')

write_descriptor(backend_instance:upper())
ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'invalid backend instance')
assert(domains.managed_persistent_domain_instances() == nil, 'invalid reload must clear prior local identity')

write_descriptor(backend_instance, ',"unknown_field":true')
ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'strict version%-1 object')

write_descriptor(backend_instance, nil, 'relative/wez-dmux.sock')
ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'invalid fixed managed socket')

write_descriptor(backend_instance, nil, nil, 'unverifiable-start-token')
ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'invalid verifiable process start token')

write_descriptor(backend_instance)
descriptor_body = descriptor_body:gsub('"socket_dev":42,', '')
ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'invalid socket device/inode witness')

write_descriptor(backend_instance)
descriptor_body = descriptor_body:gsub('macos:1700000000:123456', 'stale-boot')
ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'invalid current%-boot witness')

write_descriptor(backend_instance, nil, nil, 'linux:123456')
ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'do not match this platform')

descriptor_body = string.rep('x', 64 * 1024 + 1)
ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'invalid bounded document')

fake_wezterm.gui.dmux_read_mux_descriptor = nil
ok, err = pcall(domains.apply, { keys = {} })
assert(not ok and tostring(err):match 'maintained%-fork descriptor reader is unavailable')
fake_wezterm.gui.dmux_read_mux_descriptor = descriptor_reader

io.stdout:write 'dmux managed local domain test: descriptor identity pinned and canonical\n'
