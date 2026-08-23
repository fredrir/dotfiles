-- Acceptance case 29 (plan §13.3, §20.2): Command+Shift+T in an Archie Wez
-- pane creates on Archie with Wez even when routed by Tailscale. The Lua half
-- is that `actions.mac_keys()` binds the CMD|SHIFT t chord to the new-Space
-- prompt, and that the prompt's callback runs `space-new` through the REAL
-- controller with the pane's own marker carried byte for byte in
-- `--origin-json` — no `--backend`, no `--host`, no seam argument through
-- which the crate could be steered off the marker's owner/backend. The Rust
-- half (`gui_cli::create_space_for_origin`) selects the marker's backend and
-- dispatches a remote NEW to the marker's owner.
package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local act = setmetatable({ PopKeyTable = { name = 'PopKeyTable' } }, {
  __index = function(_, name)
    return function(value)
      value = value or {}
      value.name = name
      return value
    end
  end,
})

local spawned = {}
local errors = {}
local fake_wezterm = {
  GLOBAL = { dmux_bridge_instance = 'gui-42-cafe' },
  action = act,
  action_callback = function(callback)
    return { name = 'Callback', callback = callback }
  end,
  home_dir = '/tmp',
  log_error = function(message)
    table.insert(errors, message)
  end,
  log_warn = function() end,
  run_child_process = function(argv)
    table.insert(spawned, argv)
    return true, '{"schema_version":1,"ok":true,"result":{}}', ''
  end,
  background_child_process = function()
    error 'the new-Space prompt must not refresh in the background'
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end
package.loaded['wez.dmux_bridge.instance'] = {
  current_bridge = function()
    error 'space-new from a pane must not open a resident bridge'
  end,
  current_identity = function()
    error 'space-new from a pane must not read the resident identity'
  end,
}

local json = require 'wez.dmux_bridge.json'
local actions = require 'wez.dmux_bridge.actions'

-- The Archie marker as the pane carries it: a Wez Space on a remote owner,
-- the GUI pane attached through the Tailscale domain. Every value is the
-- exact string the pane user variables hold.
local archie = '22222222-2222-4222-8222-222222222222'
local epoch = '55555555-5555-4555-8555-555555555555'
local vars = {
  dmux_context_version = '1',
  dmux_host_uid = archie,
  dmux_space_uid = '33333333-3333-4333-8333-333333333333',
  dmux_space_no = '2',
  dmux_backend = 'wez',
  dmux_domain = 'dmux-b-tailscale',
  dmux_server_epoch = epoch,
  dmux_group_ref = 'g' .. epoch .. '.wz-7',
  dmux_split_ref = 'p' .. epoch .. '.wz-9',
}
local pane = {
  get_user_vars = function()
    return vars
  end,
  get_domain_name = function()
    return 'dmux-b-tailscale'
  end,
  pane_id = function()
    return 91
  end,
}
local window = {
  toast_notification = function() end,
}

local function binding(keys, key, mods)
  local found
  for _, item in ipairs(keys) do
    if item.key == key and item.mods == mods then
      assert(found == nil, 'duplicate binding: ' .. mods .. '+' .. key)
      found = item.action
    end
  end
  return assert(found, 'binding not found: ' .. mods .. '+' .. key)
end

local mac = actions.mac_keys()
local new_space = binding(mac, 't', 'CMD|SHIFT')
assert(new_space.name == 'PromptInputLine', 'CMD|SHIFT t must prompt for a new Space, got ' .. tostring(new_space.name))
assert(new_space.description:find('inherits active owner and backend', 1, true), new_space.description)
assert(new_space.action.name == 'Callback')
-- The plain CMD t chord is the new-Group key, not a second new-Space key.
local new_group = binding(mac, 't', 'CMD')
assert(new_group.name == 'Callback' and new_group.name ~= 'PromptInputLine')

-- An empty or cancelled prompt runs nothing.
new_space.action.callback(window, pane, '')
new_space.action.callback(window, pane, nil)
assert(#spawned == 0, 'an empty prompt must not reach the controller')

new_space.action.callback(window, pane, 'proj')
assert(#spawned == 1, 'exactly one controller call')
local argv = spawned[1]
assert(#argv == 7, 'argv is binary, _gui, --origin-json, origin, space-new, --name, NAME: ' .. #argv)
assert(argv[1] == '/tmp/.local/bin/dmux' and argv[2] == '_gui' and argv[3] == '--origin-json')
assert(argv[5] == 'space-new' and argv[6] == '--name' and argv[7] == 'proj')
for _, word in ipairs(argv) do
  for _, seam in ipairs {
    '--backend',
    '--host',
    '--socket',
    '--epoch',
    '--namespace',
    '--data-dir',
    '--lock-dir',
    '--dir',
  } do
    assert(word:sub(1, #seam) ~= seam, 'the new-Space prompt steered the crate with ' .. word)
  end
end

-- The origin is the pane's own marker, byte for byte: the owner, backend and
-- epoch the crate will create on are exactly the Archie Wez pane's.
local origin = assert(json.decode(argv[4]))
assert(origin.protocol_version == 1 and origin.gui_instance == 'gui-42-cafe')
assert(origin.pane_id == 91 and origin.domain == 'dmux-b-tailscale')
assert(origin.tmux_client_uid == nil, 'a Wez marker carries no tmux client locator')
local expected = {
  host_uid = vars.dmux_host_uid,
  space_uid = vars.dmux_space_uid,
  space_no = 2,
  backend = vars.dmux_backend,
  domain = vars.dmux_domain,
  server_epoch = vars.dmux_server_epoch,
  group_ref = vars.dmux_group_ref,
  split_ref = vars.dmux_split_ref,
}
local carried = 0
for key, value in pairs(origin.marker) do
  assert(expected[key] == value, 'marker field altered in transit: ' .. tostring(key) .. '=' .. tostring(value))
  carried = carried + 1
end
assert(carried == 8, 'the origin marker carries exactly the eight marker fields, not the GUI locator: ' .. carried)
assert(origin.marker.host_uid == archie and origin.marker.backend == 'wez')
assert(#errors == 0, 'no controller failure was logged: ' .. tostring(errors[1]))

-- The same prompt from a marker whose user variables are malformed spawns
-- nothing: the controller refuses before any child process, never guessing
-- an owner or backend.
vars.dmux_backend = 'other'
new_space.action.callback(window, pane, 'proj')
assert(#spawned == 1, 'a malformed marker must not reach the controller')
assert(#errors == 1 and errors[1]:find('space-new', 1, true), tostring(errors[1]))
vars.dmux_backend = 'wez'

io.stdout:write 'dmux mac keys test: CMD|SHIFT t creates on the pane marker owner/backend byte for byte passed\n'
