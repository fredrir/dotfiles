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
assert(tostring(err):match 'raw 32%-byte broker key unavailable')
assert(modules_loaded == 0, 'legacy modules ran before managed preflight failed')

io.stdout:write 'dmux top-level fail-closed test: missing key aborted before modules\n'
