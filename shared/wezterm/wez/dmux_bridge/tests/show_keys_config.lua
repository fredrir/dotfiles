-- Test-only dependency injection for managed_show_keys.sh. Production
-- config never accepts a descriptor path: the maintained-fork primitive
-- resolves and opens the fixed private platform-runtime descriptor itself.
local wezterm = require 'wezterm'

local root = assert(os.getenv 'DMUX_TEST_CONFIG_ROOT')
local descriptor_path = assert(os.getenv 'DMUX_TEST_DESCRIPTOR_FIXTURE')
local descriptor = assert(io.open(descriptor_path, 'rb'))
local descriptor_body = assert(descriptor:read '*a')
descriptor:close()

assert(type(wezterm.gui) == 'table')
-- The production preflight intentionally ignores DMUX_RUNTIME_DIR and reads
-- only the fixed native runtime.  This config is used solely by the isolated
-- show-keys test, so inject the already unit-tested proof while retaining the
-- real fork capability table and bridge-open surface.
wezterm.gui.dmux_bridge_preflight = function()
  return {
    version = 1,
    key_bytes = 32,
    runtime_verified = true,
    verified_mux_descriptor = true,
    launcher_witness_present = false,
  }
end
wezterm.gui.dmux_read_mux_descriptor = function(maximum)
  assert(type(maximum) == 'number' and maximum >= #descriptor_body)
  return descriptor_body
end

package.path = table.concat({
  root .. '/shared/wezterm/?.lua',
  root .. '/shared/wezterm/?/init.lua',
  package.path,
}, ';')

return dofile(root .. '/shared/wezterm/wezterm.lua')
