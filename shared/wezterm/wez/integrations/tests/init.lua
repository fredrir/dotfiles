package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local calls = {}
local fake_wezterm = {
  log_error = function(message)
    error(message)
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end

local names = {
  'wez.integrations.clean_copy',
  'wez.integrations.vscode_remote',
  'wez.integrations.ssh_hosts',
  'wez.integrations.command_palette',
}
for _, name in ipairs(names) do
  package.preload[name] = function()
    return {
      setup = function()
        calls[name] = (calls[name] or 0) + 1
      end,
    }
  end
end

require('wez.integrations').setup()

assert(calls['wez.integrations.clean_copy'] == 1)
assert(calls['wez.integrations.vscode_remote'] == 1, 'VS Code bridge must register in every mode')
if os.getenv 'DMUX_WEZ_FIRST' == '1' then
  assert(calls['wez.integrations.ssh_hosts'] == nil)
  assert(calls['wez.integrations.command_palette'] == nil)
else
  assert(calls['wez.integrations.ssh_hosts'] == 1)
  assert(calls['wez.integrations.command_palette'] == 1)
end

io.stdout:write('integration registration matches managed mode\n')

