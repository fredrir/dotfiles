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
local resident_brokered = true
local secure_bridge = {
  read_context = function(_, pane_id, maximum)
    assert(pane_id == 91 and maximum == 64 * 1024)
    return cache_document and assert(json.encode(cache_document)) or nil
  end,
  resident_brokered = function()
    return resident_brokered
  end,
}
package.loaded['wez.dmux_bridge.instance'] = {
  current_bridge = function(gui_instance)
    if gui_instance ~= 'gui-42-cafe' then
      return nil, 'wrong GUI instance'
    end
    return secure_bridge
  end,
  current_identity = function()
    return {
      gui_instance = 'gui-42-cafe',
      pid = 42,
      process_start_token = 'start-token',
    }
  end,
}

local response
local process_success = true
local process_stdout
local process_stderr = ''
local process_calls = 0
local last_argv
local toasts = {}
local errors = {}
local fake_wezterm = {
  GLOBAL = { dmux_bridge_instance = 'gui-42-cafe' },
  background_child_process = function()
    return true
  end,
  home_dir = '/tmp',
  log_error = function(message)
    table.insert(errors, message)
  end,
  log_warn = function() end,
  run_child_process = function(argv)
    assert(argv[1] == '/tmp/.local/bin/dmux')
    assert(argv[2] == '_gui' and argv[3] == '--origin-json' and type(argv[5]) == 'string')
    process_calls = process_calls + 1
    last_argv = argv
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
-- The typed error code never reaches the toast. It is the field that names
-- which refusal fired, so it must reach the log.
assert(
  errors[#errors] == 'dmux context failed (pane 91) [stale_epoch]: owner epoch changed',
  'failure log must carry the verb, origin and typed error code: ' .. tostring(errors[#errors])
)

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
assert(errors[#errors] == 'dmux space-new failed (pane 91) [partial_result]: ' .. response.message)

response.result = nil
result, err = controller.run(window, pane, 'space-new', { '--name', 'created' })
assert(result == nil and err == 'dmux controller exited unsuccessfully')

response = { schema_version = 1, ok = true, result = { refreshed = true } }
process_success = true
result = assert(controller.run(window, pane, 'context', { '--cache' }))
assert(result.refreshed == true)

local before_resident = process_calls
result = assert(controller.run_resident 'safe-quit')
assert(process_calls == before_resident + 1)
assert(#last_argv == 5 and last_argv[5] == 'safe-quit')
local resident_origin = assert(json.decode(last_argv[4]))
local resident_keys = {
  gui_instance = true,
  kind = true,
  pid = true,
  process_start_token = true,
  protocol_version = true,
}
local resident_field_count = 0
for key in pairs(resident_origin) do
  assert(resident_keys[key], 'resident origin added an unauthorized field: ' .. tostring(key))
  resident_field_count = resident_field_count + 1
end
assert(resident_field_count == 5)
assert(resident_origin.protocol_version == 1 and resident_origin.kind == 'resident_gui')
assert(resident_origin.gui_instance == 'gui-42-cafe')
assert(resident_origin.pid == 42 and resident_origin.process_start_token == 'start-token')
assert(resident_origin.pane_id == nil and resident_origin.marker == nil and resident_origin.domain == nil)

-- run_resident passes window = nil, so every toast on its failure paths is a
-- no-op. The log line is the only signal that survives it.
local before_toasts = #toasts
local saved_response = response
response = {
  schema_version = 1,
  ok = false,
  error = 'unknown_persistent_domain',
  message = 'active non-local domain is outside the sanitized dmux configuration: TermWizTerminalDomain',
}
process_success = false
result, err = controller.run_resident 'safe-quit'
assert(result == nil and err:match 'TermWizTerminalDomain')
assert(#toasts == before_toasts, 'a windowless failure cannot toast')
assert(
  errors[#errors] == 'dmux safe-quit failed (resident_gui) [unknown_persistent_domain]: ' .. response.message,
  'a windowless failure must still log: ' .. tostring(errors[#errors])
)
response = saved_response
process_success = true

before_resident = process_calls
result, err = controller.run_resident 'context'
assert(result == nil and err:match 'restricted to safe%-quit' and process_calls == before_resident)
result, err = controller.run_resident('safe-quit', { '--force' })
assert(result == nil and err:match 'does not accept arguments' and process_calls == before_resident)
resident_brokered = false
result, err = controller.run_resident 'safe-quit'
assert(result == nil and err:match 'not broker%-established' and process_calls == before_resident)
resident_brokered = true

process_success = false
process_stdout = 'NOT-JSON'
process_stderr = 'plain failure\n'
result, err = controller.run(window, pane, 'context', { '--cache' })
assert(result == nil and err == 'plain failure')

assert(errors[#errors] == 'dmux context failed (pane 91): plain failure')

process_stdout = nil
response = { schema_version = 1, ok = true, result = {} }
result, err = controller.run(window, pane, 'context', { '--cache' })
assert(result == nil and err:match 'success JSON with an unsuccessful exit status')
assert(errors[#errors] == 'dmux context failed (pane 91): ' .. err)

-- An unresolvable marker has no origin to name, so the scope is omitted rather
-- than reported as a pane that was never proved.
local context_module = package.loaded['wez.dmux_bridge.context']
local resolved_marker = context_module.from_pane
context_module.from_pane = function()
  return nil, { message = 'pane has no dmux marker' }
end
result, err = controller.run(window, pane, 'space-new')
assert(result == nil and err == 'pane has no dmux marker')
assert(errors[#errors] == 'dmux space-new failed: pane has no dmux marker')
context_module.from_pane = resolved_marker

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

io.stdout:write 'dmux controller/cache test: typed failures, resident origin, and exact cache passed\n'
