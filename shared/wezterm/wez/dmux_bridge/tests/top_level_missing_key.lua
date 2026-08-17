package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

assert(os.getenv 'DMUX_WEZ_FIRST' == '1')
local modules_loaded = 0
local fake_wezterm = {
  GLOBAL = {},
  config_builder = function()
    return {}
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
      error 'dmux_bridge_key_unavailable'
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
  'wez.domains',
  'wez.remote',
  'wez.integrations',
  'wez.plugins',
} do
  package.preload[name] = function()
    modules_loaded = modules_loaded + 1
    return {}
  end
end

local ok, err = pcall(dofile, 'shared/wezterm/wezterm.lua')
assert(not ok, 'missing broker key must abort managed config evaluation')
assert(tostring(err):match 'dmux_bridge_key_unavailable')
assert(modules_loaded == 0, 'legacy modules ran before managed preflight failed')
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domains == nil)
assert(fake_wezterm.GLOBAL.dmux_managed_persistent_domain_instances == nil)

io.stdout:write 'dmux top-level fail-closed test: missing key aborted before modules\n'
