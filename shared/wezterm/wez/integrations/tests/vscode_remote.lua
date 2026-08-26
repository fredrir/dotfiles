package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local events = {}
local spawns = {}
local errors = {}
local fail_spawn = false
local fake_wezterm = {
  background_child_process = function(args)
    if fail_spawn then
      error 'deliberate spawn failure'
    end
    table.insert(spawns, args)
  end,
  log_error = function(message)
    table.insert(errors, message)
  end,
  on = function(name, callback)
    events[name] = callback
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end
package.preload['wez.platform'] = function()
  return {
    pick = function(choices)
      return choices.mac
    end,
  }
end

local integration = require 'wez.integrations.vscode_remote'
integration.setup()

local handler = assert(events['user-var-changed'], 'integration must register user-var-changed')
handler({}, {}, 'some_other_var', 'archie\n1.1\n/home/fredrir')
assert(#spawns == 0, 'unrelated user vars must be ignored')

handler({}, {}, 'vscode_remote_open', 'archie\n123.1\n/home/fredrir/project with spaces')
assert(#spawns == 1, 'valid Archie request must launch once')
assert(spawns[1][1] == '/usr/local/bin/code')
assert(spawns[1][2] == '--remote')
assert(spawns[1][3] == 'ssh-remote+archie')
assert(spawns[1][4] == '/home/fredrir/project with spaces')
assert(spawns[1][5] == nil, 'request values must be passed as four direct arguments')

local newline_path = '/Users/fredrir/project\nwith-newline'
handler({}, {}, 'vscode_remote_open', 'macie\n123.2\n' .. newline_path)
assert(#spawns == 2, 'valid Macie request must launch once')
assert(spawns[2][3] == 'ssh-remote+macie' and spawns[2][4] == newline_path)

local invalid = {
  'other\n123.3\n/home/fredrir',
  'archie\nnot-a-nonce\n/home/fredrir',
  'archie\n123.4\nrelative/path',
  'archie\n123.5\n',
  'archie\n123.6\n/home/fredrir\0suffix',
  string.rep('x', 9000),
}
for _, value in ipairs(invalid) do
  handler({}, {}, 'vscode_remote_open', value)
end
assert(#spawns == 2, 'malformed requests must not launch VS Code')

fail_spawn = true
handler({}, {}, 'vscode_remote_open', 'archie\n123.7\n/home/fredrir')
assert(#spawns == 2, 'failed child process must not be recorded as launched')
assert(#errors == 1 and errors[1]:find('VS Code', 1, true), 'spawn failure must be logged without escaping the event')

io.stdout:write 'WezTerm validates requests and launches VS Code without a shell\n'

