package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local marker = {
  host_uid = '22222222-2222-4222-8222-222222222222',
  space_uid = '33333333-3333-4333-8333-333333333333',
  space_no = 2,
  backend = 'wez',
  domain = nil,
  gui_domain = 'dmux-b-usb',
  gui_pane_id = 91,
  server_epoch = '55555555-5555-4555-8555-555555555555',
  group_ref = 'g55555555-5555-4555-8555-555555555555.wz-7',
  split_ref = 'p55555555-5555-4555-8555-555555555555.wz-9',
}
local function marker_context(value)
  return {
    host_uid = value.host_uid,
    space_uid = value.space_uid,
    space_no = value.space_no,
    backend = value.backend,
    server_epoch = value.server_epoch,
    group_ref = value.group_ref,
    split_ref = value.split_ref,
  }
end
package.loaded['wez.dmux_bridge.context'] = {
  from_pane = function()
    return marker
  end,
  marker_context = marker_context,
}

local cache_document
local json
package.loaded['wez.dmux_bridge.fs'] = {
  join = function(...)
    return table.concat({ ... }, '/')
  end,
  read = function(path)
    assert(path == '/runtime/bridge/instances/gui-42-cafe/context/91.json')
    return cache_document and assert(json.encode(cache_document)) or nil
  end,
}
package.loaded['wez.dmux_bridge.instance'] = {
  runtime_dir = function()
    return '/runtime'
  end,
}

local response
local process_success = true
local process_stdout
local process_stderr = ''
local toasts = {}
local fake_wezterm = {
  GLOBAL = { dmux_bridge_instance = 'gui-42-cafe' },
  background_child_process = function()
    return true
  end,
  home_dir = '/tmp',
  log_warn = function() end,
  run_child_process = function(argv)
    assert(argv[1] == '/tmp/.local/bin/dmux')
    assert(argv[2] == '_gui' and argv[3] == '--origin-json' and type(argv[5]) == 'string')
    return process_success, process_stdout or assert(json.encode(response)), process_stderr
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end

json = require 'wez.dmux_bridge.json'
local controller = require 'wez.dmux_bridge.controller'
local window = {
  toast_notification = function(_, _, message)
    table.insert(toasts, message)
  end,
}
local pane = {}

response = { schema_version = 1, ok = false, error = 'stale_epoch', message = 'owner epoch changed' }
process_success = false
local result, err = controller.run(window, pane, 'context', { '--cache' })
assert(result == nil and err == 'owner epoch changed')
assert(toasts[#toasts] == 'owner epoch changed', 'typed JSON failure must win over stderr/exit status')

response = {
  schema_version = 1,
  ok = false,
  error = 'partial_result',
  message = 'Space b3 was created but GUI presentation failed',
  result = { ref = 'b3', created = true, connected = false },
}
local partial
result, err, partial = controller.run(window, pane, 'space-new', { '--name', 'created' })
assert(result == nil and err:match 'was created')
assert(partial and partial.ref == 'b3' and partial.created == true and partial.connected == false)

response.result = nil
result, err = controller.run(window, pane, 'space-new', { '--name', 'created' })
assert(result == nil and err == 'dmux controller exited unsuccessfully')

response = { schema_version = 1, ok = true, result = { refreshed = true } }
process_success = true
result = assert(controller.run(window, pane, 'context', { '--cache' }))
assert(result.refreshed == true)

process_success = false
process_stdout = 'NOT-JSON'
process_stderr = 'plain failure\n'
result, err = controller.run(window, pane, 'context', { '--cache' })
assert(result == nil and err == 'plain failure')

process_stdout = nil
response = { schema_version = 1, ok = true, result = {} }
result, err = controller.run(window, pane, 'context', { '--cache' })
assert(result == nil and err:match 'success JSON with an unsuccessful exit status')

local now = os.time()
cache_document = {
  schema_version = 1,
  gui_instance = 'gui-42-cafe',
  pane_id = 91,
  validated_at = now,
  ok = true,
  marker = marker_context(marker),
  display = {
    logical_ref = 'b:2',
    space_name = 'work',
    backend = 'wez',
    owner_alias = 'b',
    owner_label = 'Archie',
    route = 'usb',
    group_count = 2,
    split_count = 3,
    group_name = 'editor',
  },
}
local cached = assert(controller.cached_context(pane, now))
assert(cached.display.owner_label == 'Archie')

cache_document.marker.space_uid = '77777777-7777-4777-8777-777777777777'
local _, _, state = controller.cached_context(pane, now)
assert(state == 'unverified')

cache_document.marker = marker_context(marker)
cache_document.unknown = true
_, _, state = controller.cached_context(pane, now)
assert(state == 'invalid_cache')

cache_document.unknown = nil
cache_document.ok = false
cache_document.display = nil
cache_document.error = 'stale_epoch'
cache_document.message = 'owner epoch changed'
_, err, state = controller.cached_context(pane, now)
assert(err == 'owner epoch changed' and state == 'invalid_context')

io.stdout:write 'dmux controller/cache test: typed failures and exact cache passed\n'
