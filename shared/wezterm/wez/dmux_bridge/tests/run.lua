package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local canonical = require 'wez.dmux_bridge.canonical'
local context = require 'wez.dmux_bridge.context'
local correlation = require 'wez.dmux_bridge.correlation'
local crypto = require 'wez.dmux_bridge.crypto'
local json = require 'wez.dmux_bridge.json'
local protocol = require 'wez.dmux_bridge.protocol'

local tests = 0

local function equal(actual, expected, label)
  tests = tests + 1
  if actual ~= expected then
    error(string.format('%s\nexpected: %s\nactual:   %s', label, tostring(expected), tostring(actual)), 2)
  end
end

local function truthy(value, label)
  tests = tests + 1
  if not value then
    error(label, 2)
  end
end

local function error_code(_, err)
  return err and err.code
end

-- FIPS/RFC vectors pin the dependency-free implementation.
equal(crypto.sha256 '', 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855', 'sha256 empty')
equal(crypto.sha256 'abc', 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad', 'sha256 abc')
equal(
  crypto.hmac_sha256(string.rep(string.char(0x0b), 20), 'Hi There'),
  'b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7',
  'RFC 4231 HMAC case 1'
)

local host = '22222222-2222-4222-8222-222222222222'
local space = '33333333-3333-4333-8333-333333333333'
local instance_uid = '44444444-4444-4444-8444-444444444444'
local epoch = '55555555-5555-4555-8555-555555555555'
local group = 'g' .. epoch .. '.wz-7'
local split = 'p' .. epoch .. '.wz-9'
local key = '0123456789abcdef0123456789abcdef'

local request = {
  protocol_version = 1,
  uid = '11111111-1111-4111-8111-111111111111',
  action = 'present',
  target = {
    domain = 'dmux-b-usb',
    workspace = 'dmux:' .. host .. ':' .. space,
    host_uid = host,
    space_uid = space,
    backend_instance_uid = instance_uid,
    server_epoch = epoch,
    group_ref = group,
    split_ref = split,
    alternate_domains = { 'dmux-b-ts' },
  },
  issued_at = 1800000000,
  expiry = 1800000010,
  nonce = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  replay_key = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
  origin = {
    kind = 'in_gui',
    gui_instance = 'gui-42-cafe',
    pid = 42,
    process_start_token = 'start-token',
    pane_id = 91,
    domain = 'dmux-b-usb',
    host_uid = host,
    space_uid = space,
    space_no = 7,
    backend = 'wez',
    server_epoch = epoch,
    group_ref = group,
    split_ref = split,
  },
}

local expected_document = '{"action":"present","expiry":1800000010,"issued_at":1800000000,'
  .. '"nonce":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","origin":{"backend":"wez","domain":"dmux-b-usb",'
  .. '"group_ref":"g55555555-5555-4555-8555-555555555555.wz-7","gui_instance":"gui-42-cafe",'
  .. '"host_uid":"22222222-2222-4222-8222-222222222222","kind":"in_gui","pane_id":91,"pid":42,'
  .. '"process_start_token":"start-token","server_epoch":"55555555-5555-4555-8555-555555555555",'
  .. '"space_no":7,"space_uid":"33333333-3333-4333-8333-333333333333",'
  .. '"split_ref":"p55555555-5555-4555-8555-555555555555.wz-9"},"protocol_version":1,'
  .. '"replay_key":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","target":{"alternate_domains":["dmux-b-ts"],'
  .. '"backend_instance_uid":"44444444-4444-4444-8444-444444444444","domain":"dmux-b-usb",'
  .. '"group_ref":"g55555555-5555-4555-8555-555555555555.wz-7",'
  .. '"host_uid":"22222222-2222-4222-8222-222222222222",'
  .. '"server_epoch":"55555555-5555-4555-8555-555555555555",'
  .. '"space_uid":"33333333-3333-4333-8333-333333333333",'
  .. '"split_ref":"p55555555-5555-4555-8555-555555555555.wz-9",'
  .. '"workspace":"dmux:22222222-2222-4222-8222-222222222222:'
  .. '33333333-3333-4333-8333-333333333333"},"uid":"11111111-1111-4111-8111-111111111111"}'

local _, document = protocol.sign(request, key)
equal(document, expected_document, 'cross-language canonical signing document')
equal(crypto.sha256(document), '61f7c94cb55544583d28a47ee48684a14921a0760d5217f26cbcfad984a069df', 'document sha256')
equal(request.hmac_sha256, '293f019ae3ee7a55784dd6d03593e431f0d64117e496703ac7e39ed7ebcfce80', 'document HMAC')
local authenticated, auth_err = protocol.validate_and_authenticate(request, key, 1800000000, 'gui-42-cafe')
truthy(authenticated and not auth_err, 'signed request authenticates')

-- Empty array/object shape is part of the signed schema. This vector pins
-- the sole v1 empty-array authorization used by a tmux-only safe-quit proof.
local empty_quit = {
  protocol_version = 1,
  uid = request.uid,
  action = 'safe_quit',
  target = { phase = 'detach', domains = json.array() },
  issued_at = request.issued_at,
  expiry = request.expiry,
  nonce = request.nonce,
  replay_key = request.replay_key,
  origin = request.origin,
}
local _, empty_document = protocol.sign(empty_quit, key)
equal(
  crypto.sha256(empty_document),
  '9020e21a60eb17b851792de5bf1f9bd90174bbe4dd7120be0895d57d9fda5e15',
  'empty safe-quit document sha256'
)
equal(
  empty_quit.hmac_sha256,
  'a188cd6cc180e07fcfc2ba88e5609885c732ac0e7748e9b451cfcc73465958b9',
  'empty safe-quit document HMAC'
)
local empty_wire = assert(json.encode(empty_quit))
local empty_round_trip = assert(json.decode(empty_wire))
truthy(json.is_array(empty_round_trip.target.domains), 'strict codec preserves empty JSON array shape')
truthy(
  protocol.validate_and_authenticate(empty_round_trip, key, 1800000000, 'gui-42-cafe'),
  'empty safe-quit array authenticates after wire round trip'
)
local function clone(value)
  if type(value) ~= 'table' then
    return value
  end
  local out = {}
  for key_name, item in pairs(value) do
    out[key_name] = clone(item)
  end
  return out
end

local repeated_replay_key = clone(request)
repeated_replay_key.uid = '99999999-9999-4999-8999-999999999999'
protocol.sign(repeated_replay_key, key)
truthy(
  protocol.validate_and_authenticate(repeated_replay_key, key, 1800000000, 'gui-42-cafe'),
  'a replay key repeated under a different request UID is not a second one-use identity'
)

-- A syntactically valid object cannot be reinterpreted as the authorized
-- empty array, even if every other signed field is well formed.
local object_domains = clone(empty_quit)
object_domains.target.domains = {}
protocol.sign(object_domains, key)
equal(
  error_code(protocol.validate_and_authenticate(object_domains, key, 1800000000, 'gui-42-cafe')),
  'malformed_request',
  'empty object is not an empty domains array'
)

local duplicate, duplicate_err = json.decode '{"a":1,"a":2}'
truthy(not duplicate and duplicate_err:match 'duplicate object key', 'duplicate JSON object keys rejected')
local null_value = json.decode '{"a":null}'
truthy(not null_value, 'JSON null rejected from protocol subset')
local fraction = json.decode '{"a":1.5}'
truthy(not fraction, 'fractional JSON number rejected from protocol subset')

local tampered = clone(request)
tampered.target.alternate_domains[1] = 'dmux-b-other'
equal(
  error_code(protocol.validate_and_authenticate(tampered, key, 1800000000, 'gui-42-cafe')),
  'unauthorized',
  'tamper fails HMAC'
)
equal(
  error_code(protocol.validate_and_authenticate(request, key, 1800000011, 'gui-42-cafe')),
  'expired',
  'expired request rejected'
)
local wrong_instance = clone(request)
wrong_instance.origin.gui_instance = 'gui-99-beef'
protocol.sign(wrong_instance, key)
equal(
  error_code(protocol.validate_and_authenticate(wrong_instance, key, 1800000000, 'gui-42-cafe')),
  'invalid_origin',
  'origin instance mismatch rejected'
)
local stale_child = clone(request)
stale_child.target.group_ref = 'g66666666-6666-4666-8666-666666666666.wz-7'
protocol.sign(stale_child, key)
equal(
  error_code(protocol.validate_and_authenticate(stale_child, key, 1800000000, 'gui-42-cafe')),
  'malformed_request',
  'child epoch mismatch rejected'
)
local leading_zero = clone(request)
leading_zero.target.group_ref = 'g' .. epoch .. '.wz-07'
protocol.sign(leading_zero, key)
equal(
  error_code(protocol.validate_and_authenticate(leading_zero, key, 1800000000, 'gui-42-cafe')),
  'malformed_request',
  'noncanonical child handle rejected'
)

local wrong_workspace = clone(request)
wrong_workspace.target.workspace = 'dmux:' .. host .. ':77777777-7777-4777-8777-777777777777'
protocol.sign(wrong_workspace, key)
equal(
  error_code(protocol.validate_and_authenticate(wrong_workspace, key, 1800000000, 'gui-42-cafe')),
  'malformed_request',
  'opaque workspace must bind exact HostUid and SpaceUid'
)

local split_without_group = clone(request)
split_without_group.target.group_ref = nil
protocol.sign(split_without_group, key)
equal(
  error_code(protocol.validate_and_authenticate(split_without_group, key, 1800000000, 'gui-42-cafe')),
  'malformed_request',
  'Split target requires its parent Group'
)

local attach_self_alternate = clone(request)
attach_self_alternate.action = 'attach_domain'
attach_self_alternate.target = {
  domain = 'dmux-b-usb',
  backend_instance_uid = instance_uid,
  server_epoch = epoch,
  alternate_domains = { 'dmux-b-usb' },
}
protocol.sign(attach_self_alternate, key)
equal(
  error_code(protocol.validate_and_authenticate(attach_self_alternate, key, 1800000000, 'gui-42-cafe')),
  'malformed_request',
  'attach alternate cannot name selected domain'
)

local safe_detach_extra = clone(request)
safe_detach_extra.action = 'safe_quit'
safe_detach_extra.target = {
  phase = 'detach',
  domains = { 'dmux-b-usb' },
  proof_uid = '11111111-1111-4111-8111-111111111111',
}
protocol.sign(safe_detach_extra, key)
equal(
  error_code(protocol.validate_and_authenticate(safe_detach_extra, key, 1800000000, 'gui-42-cafe')),
  'malformed_request',
  'safe quit detach phase rejects finish fields'
)

local cold = clone(request)
cold.origin = {
  kind = 'cold_launcher',
  gui_instance = 'gui-42-cafe',
  uid = 501,
  pid = 4242,
  start_token = 'Sun Aug 16 12:34:56 2026',
  launcher_request_uid = '88888888-8888-4888-8888-888888888888',
  domain = 'dmux-b-usb',
  host_uid = host,
  backend_instance_uid = instance_uid,
  server_epoch = epoch,
  space_uid = space,
}
protocol.sign(cold, key)
truthy(
  protocol.validate_and_authenticate(cold, key, 1800000000, 'gui-42-cafe'),
  'bound cold presentation authenticates'
)

local cold_wrong_domain = clone(cold)
cold_wrong_domain.target.domain = 'dmux-b-ts'
cold_wrong_domain.target.alternate_domains = nil
protocol.sign(cold_wrong_domain, key)
equal(
  error_code(protocol.validate_and_authenticate(cold_wrong_domain, key, 1800000000, 'gui-42-cafe')),
  'invalid_origin',
  'cold origin binds exact target domain/backend instance'
)

local cold_detach = clone(cold)
cold_detach.action = 'detach_domain'
cold_detach.target = {
  domain = 'dmux-b-usb',
  backend_instance_uid = instance_uid,
  server_epoch = epoch,
}
protocol.sign(cold_detach, key)
equal(
  error_code(protocol.validate_and_authenticate(cold_detach, key, 1800000000, 'gui-42-cafe')),
  'origin_not_allowed',
  'cold launcher cannot detach a domain'
)

local cold_zero_pid = clone(cold)
cold_zero_pid.origin.pid = 0
protocol.sign(cold_zero_pid, key)
equal(
  error_code(protocol.validate_and_authenticate(cold_zero_pid, key, 1800000000, 'gui-42-cafe')),
  'invalid_origin',
  'cold launcher PID is nonzero'
)

local vars = {
  dmux_context_version = '1',
  dmux_host_uid = host,
  dmux_space_uid = space,
  dmux_space_no = '2',
  dmux_backend = 'wez',
  dmux_domain = '',
  dmux_server_epoch = epoch,
  dmux_group_ref = group,
  dmux_split_ref = split,
}

local marker = context.parse_vars(vars, 'dmux-b-usb', 91)
truthy(marker, 'lowercase v1 marker parses')
equal(marker.gui_domain, 'dmux-b-usb', 'actual GUI domain retained')
equal(context.space_uri(marker), 'dmux://' .. host .. '/spaces/' .. space, 'canonical Space URI')
equal(
  context.group_uri(marker),
  'dmux://' .. host .. '/spaces/' .. space .. '/groups/' .. epoch .. '/wz-7',
  'canonical Group URI'
)

local bad_domain = clone(vars)
bad_domain.dmux_domain = 'another-domain'
equal(error_code(context.parse_vars(bad_domain, 'dmux-b-usb', 91)), 'marker_domain_mismatch', 'marker domain mismatch')
local huge_space_no = clone(vars)
huge_space_no.dmux_space_no = '9007199254740992'
equal(
  error_code(context.parse_vars(huge_space_no, 'dmux-b-usb', 91)),
  'malformed_marker',
  'Space number above the exact JSON integer range is rejected'
)
equal(
  error_code(context.parse_vars(vars, 'dmux-b-usb', 9007199254740992)),
  'malformed_marker',
  'pane id above the exact JSON integer range is rejected'
)
local bad_backend = clone(vars)
bad_backend.dmux_backend = 'tmux'
equal(
  error_code(context.parse_vars(bad_backend, 'dmux-b-usb', 91)),
  'marker_backend_mismatch',
  'marker backend mismatch'
)
local uppercase_only = {}
for name, value in pairs(vars) do
  uppercase_only[name:upper()] = value
end
equal(
  error_code(context.parse_vars(uppercase_only, 'dmux-b-usb', 91)),
  'missing_marker',
  'uppercase aliases not guessed'
)

local function pane(id, pane_vars, domain)
  local active = false
  return {
    pane_id = function()
      return id
    end,
    get_user_vars = function()
      return pane_vars
    end,
    get_domain_name = function()
      return domain or 'dmux-b-usb'
    end,
    activate = function()
      active = true
    end,
    was_activated = function()
      return active
    end,
  }
end

local function tab(id, pane_infos)
  local activated = false
  return {
    tab_id = function()
      return id
    end,
    panes_with_info = function()
      return pane_infos
    end,
    panes = function()
      local out = {}
      for _, info in ipairs(pane_infos) do
        table.insert(out, info.pane)
      end
      return out
    end,
    activate = function()
      activated = true
    end,
    was_activated = function()
      return activated
    end,
  }
end

local function window(id, workspace, tabs)
  return {
    window_id = function()
      return id
    end,
    get_workspace = function()
      return workspace
    end,
    tabs = function()
      return tabs
    end,
  }
end

local p1 = pane(91, vars)
local vars2 = clone(vars)
vars2.dmux_split_ref = 'p' .. epoch .. '.wz-10'
local p2 = pane(92, vars2)
local t1 = tab(7, { { pane = p1, is_active = false }, { pane = p2, is_active = true } })
local w1 = window(5, request.target.workspace, { t1 })
local active_workspace
local mock_mux = {
  all_windows = function()
    return { w1 }
  end,
  set_active_workspace = function(name)
    active_workspace = name
  end,
}

local correlated = correlation.resolve(mock_mux, request.target)
equal(correlated.pane_id, 91, 'exact Split ref selects exact GUI-local pane')
local group_target = clone(request.target)
group_target.split_ref = nil
correlated = correlation.resolve(mock_mux, group_target)
equal(correlated.pane_id, 92, 'Group-only preserves active matching pane')
correlated = correlation.activate(mock_mux, request.target)
equal(active_workspace, request.target.workspace, 'activate uses set_active_workspace')
truthy(p1:was_activated() and t1:was_activated(), 'correlated tab and pane focused')

local duplicate = tab(8, { { pane = pane(93, vars), is_active = false } })
local duplicate_window = window(5, request.target.workspace, { t1, duplicate })
mock_mux.all_windows = function()
  return { duplicate_window }
end
equal(error_code(correlation.resolve(mock_mux, request.target)), 'ambiguous_group', 'cross-tab Group marker rejected')

local another_window = window(6, request.target.workspace, { t1 })
mock_mux.all_windows = function()
  return { w1, another_window }
end
equal(error_code(correlation.resolve(mock_mux, request.target)), 'ambiguous_workspace', 'duplicate workspace rejected')

local invalid_marker_window =
  window(5, request.target.workspace, { tab(10, { { pane = pane(96, {}), is_active = true } }) })
mock_mux.all_windows = function()
  return { invalid_marker_window }
end
equal(
  error_code(correlation.resolve(mock_mux, request.target)),
  'invalid_marker',
  'invalid target pane marker fails closed'
)

local wrong_space_vars = clone(vars)
wrong_space_vars.dmux_space_uid = '77777777-7777-4777-8777-777777777777'
local wrong_space_window =
  window(5, request.target.workspace, { tab(11, { { pane = pane(97, wrong_space_vars), is_active = true } }) })
mock_mux.all_windows = function()
  return { wrong_space_window }
end
equal(
  error_code(correlation.resolve(mock_mux, request.target)),
  'workspace_context_mismatch',
  'cross-Space pane inside opaque workspace fails closed'
)

local other_vars = clone(vars)
other_vars.dmux_host_uid = '77777777-7777-4777-8777-777777777777'
local mixed_tab =
  tab(9, { { pane = pane(94, vars), is_active = true }, { pane = pane(95, other_vars), is_active = false } })
truthy(context.tab_summary(mixed_tab).mixed, 'different logical owners make a tab MIXED')
mock_mux.all_windows = function()
  return { window(5, request.target.workspace, { mixed_tab }) }
end
correlated = correlation.resolve(mock_mux, request.target)
equal(correlated.pane_id, 94, 'valid MIXED tab still correlates the exact active-marker Split')

local other_group_vars = clone(vars)
other_group_vars.dmux_group_ref = 'g' .. epoch .. '.wz-8'
local mixed_group_tab = tab(12, {
  { pane = pane(98, vars), is_active = true },
  { pane = pane(99, other_group_vars), is_active = false },
})
truthy(context.tab_summary(mixed_group_tab).mixed, 'different logical Groups in one physical tab are MIXED')

local escaped, escape_err = canonical.encode { text = 'quote"\n\0' }
equal(escape_err, nil, 'canonical string escapes encode')
equal(escaped, '{"text":"quote\\"\\n\\u0000"}', 'canonical RFC8259 escapes pinned')

io.stdout:write(string.format('dmux bridge lua tests: %d passed\n', tests))
