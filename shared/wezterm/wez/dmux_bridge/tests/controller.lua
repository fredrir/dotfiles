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
local background_calls = 0
local last_background_argv
local fake_wezterm = {
  GLOBAL = { dmux_bridge_instance = 'gui-42-cafe' },
  background_child_process = function(argv)
    background_calls = background_calls + 1
    last_background_argv = argv
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

-- ADR 012 WS-E.2, controller.lua:115 (report 08 §7). The controller is the
-- carrier of the pane marker's server_epoch into `dmux _gui --origin-json`
-- and verifies nothing about the epoch itself: plan §13.1 puts that in the
-- crate (`operations::validate_marker_context`). What it must hold is that
-- the origin is the pane's own marker, byte for byte and nothing more — no
-- synthesized or defaulted epoch, no socket, namespace or seam argument
-- through which the crate's verification could be steered, no environment
-- lookup beyond the binary's path, and no child process at all once the
-- marker cannot be parsed.
assert(#last_argv == 6, 'argv is binary, _gui, --origin-json, origin, verb and one argument')
assert(last_argv[5] == 'context' and last_argv[6] == '--cache')
local in_gui_origin = assert(json.decode(last_argv[4]))
local in_gui_keys = { domain = true, gui_instance = true, marker = true, pane_id = true, protocol_version = true }
local in_gui_field_count = 0
for key in pairs(in_gui_origin) do
  assert(in_gui_keys[key], 'in-GUI origin added an unauthorized field: ' .. tostring(key))
  in_gui_field_count = in_gui_field_count + 1
end
assert(in_gui_field_count == 5)
assert(in_gui_origin.protocol_version == 1 and in_gui_origin.gui_instance == 'gui-42-cafe')
assert(in_gui_origin.pane_id == 91 and in_gui_origin.domain == 'dmux-b-usb')
local carried = in_gui_origin.marker
local carried_count = 0
for key, value in pairs(carried) do
  assert(marker[key] == value, 'marker field altered in transit: ' .. tostring(key))
  carried_count = carried_count + 1
end
assert(carried_count == 7, 'the origin marker carries exactly the seven marker fields, not the GUI locator')
assert(carried.server_epoch == marker.server_epoch and carried.group_ref == marker.group_ref)
assert(carried.domain == nil, 'an absent provider domain is omitted, never guessed from the GUI domain')
assert(in_gui_origin.tmux_client_uid == nil, 'a Wez marker carries no tmux client locator')
for _, word in ipairs(last_argv) do
  for _, seam in ipairs { '--socket', '--epoch', '--namespace', '--data-dir', '--lock-dir', '--backend' } do
    assert(word:sub(1, #seam) ~= seam, 'seam argument reached the controller argv: ' .. word)
  end
end

-- A tmux marker's client locator rides beside the marker, never inside it.
marker.tmux_client_uid = '66666666-6666-4666-8666-666666666666'
assert(controller.run(window, pane, 'context', { '--cache' }))
in_gui_origin = assert(json.decode(last_argv[4]))
assert(in_gui_origin.tmux_client_uid == '66666666-6666-4666-8666-666666666666')
assert(in_gui_origin.marker.tmux_client_uid == nil)
marker.tmux_client_uid = nil

-- The controller reads nothing from the process environment but the
-- binary's location: no epoch, socket or policy can enter from there.
-- (`rawset`: the probe replaces a stdlib field for one call and restores
-- it; luacheck would otherwise read the assignment as a stray global edit.)
local real_getenv = os.getenv
local env_reads = {}
rawset(os, 'getenv', function(name)
  env_reads[name] = true
  return real_getenv(name)
end)
assert(controller.run(window, pane, 'split-new', { '--direction', 'right' }))
rawset(os, 'getenv', real_getenv)
for name in pairs(env_reads) do
  assert(name == 'DMUX_BIN', 'controller consulted the environment for ' .. tostring(name))
end
assert(env_reads.DMUX_BIN, 'the binary is resolved through DMUX_BIN')

-- The epoch-class refusal the crate answers with once its verification
-- fails (`backend_epoch_changed`) is a failure here, with no result and the
-- code in the log; the controller never retries, rewrites or softens it.
local before_epoch_refusal = process_calls
response = {
  schema_version = 1,
  ok = false,
  error = 'backend_epoch_changed',
  message = 'backend instance has published no server epoch',
}
process_success = false
result, err, partial = controller.run(window, pane, 'group-new')
assert(result == nil and err == response.message and partial == nil)
assert(process_calls == before_epoch_refusal + 1)
assert(errors[#errors] == 'dmux group-new failed (pane 91) [backend_epoch_changed]: ' .. response.message)
response = { schema_version = 1, ok = true, result = { refreshed = true } }
process_success = true

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
local before_unparseable = process_calls
local before_background = background_calls
result, err = controller.run(window, pane, 'space-new')
assert(result == nil and err == 'pane has no dmux marker')
assert(errors[#errors] == 'dmux space-new failed: pane has no dmux marker')
-- An unparseable marker is the end of the road: no origin is fabricated
-- from the GUI's own identity, so no child process runs, foreground or
-- background.
assert(process_calls == before_unparseable, 'an unparseable marker reached the controller binary')
assert(controller.background_refresh(pane) == false)
assert(background_calls == before_background, 'an unparseable marker started a background refresh')
local gui_instance_before = fake_wezterm.GLOBAL.dmux_bridge_instance
fake_wezterm.GLOBAL.dmux_bridge_instance = nil
context_module.from_pane = resolved_marker
-- And a parseable marker without a ready GUI bridge identity is refused
-- before the binary too: the origin is never half-built.
result, err = controller.run(window, pane, 'space-new')
assert(result == nil and err == 'trusted dmux GUI bridge is not ready')
assert(process_calls == before_unparseable)
fake_wezterm.GLOBAL.dmux_bridge_instance = gui_instance_before
assert(controller.background_refresh(pane) == true)
assert(background_calls == before_background + 1)
assert(#last_background_argv == 6 and last_background_argv[5] == 'context' and last_background_argv[6] == '--cache')
assert(json.decode(last_background_argv[4]).marker.server_epoch == marker.server_epoch)

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

io.stdout:write 'dmux controller/cache test: typed failures, exact origin carriage, resident origin, and exact cache passed\n'
