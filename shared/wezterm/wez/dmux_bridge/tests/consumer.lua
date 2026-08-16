package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local root = assert(os.getenv 'DMUX_CONSUMER_TEST_DIR')
local requests = root .. '/requests'
local acks = root .. '/acks'
local consumed = root .. '/consumed'
assert(os.execute(string.format('/bin/mkdir -p %q %q %q', requests, acks, consumed)))

local function clone(value)
  if type(value) ~= 'table' then
    return value
  end
  local out = {}
  for key, item in pairs(value) do
    out[key] = clone(item)
  end
  return out
end

local known_requests = {}
local scheduled = {}
local errors = {}

local fake_wezterm = {
  GLOBAL = {},
  log_error = function(message)
    table.insert(errors, message)
  end,
  read_dir = function()
    local out = {}
    for path in pairs(known_requests) do
      local file = io.open(path, 'rb')
      if file then
        file:close()
        table.insert(out, path)
      end
    end
    return out
  end,
  time = {
    call_after = function(_, callback)
      table.insert(scheduled, callback)
    end,
  },
}
package.preload.wezterm = function()
  return fake_wezterm
end

local fake_fs = {}
function fake_fs.join(...)
  return table.concat({ ... }, '/')
end
function fake_fs.read(path, maximum)
  local file = io.open(path, 'rb')
  if not file then
    return nil, 'not_found'
  end
  local body = file:read '*a'
  file:close()
  if maximum and #body > maximum then
    return nil, 'too_large'
  end
  return body
end
function fake_fs.write_private_atomic(path, body)
  local file = assert(io.open(path, 'wb'))
  assert(file:write(body))
  file:close()
  return true
end
package.loaded['wez.dmux_bridge.fs'] = fake_fs

local heartbeat_count = 0
local state = {
  id = 'gui-42-cafe',
  key = '0123456789abcdef0123456789abcdef',
  paths = { requests = requests, acks = acks, consumed = consumed },
  safe_quit = {},
}
package.loaded['wez.dmux_bridge.instance'] = {
  create = function()
    return state
  end,
  heartbeat = function()
    heartbeat_count = heartbeat_count + 1
    return true
  end,
}

local dispatch_count = 0
local throw_once = false
package.loaded['wez.dmux_bridge.presentation'] = {
  dispatch = function(request, _, done)
    dispatch_count = dispatch_count + 1
    if throw_once then
      throw_once = false
      error 'deterministic dispatch failure'
    end
    assert(request.action == 'ping')
    done { pong = true }
  end,
}

local protocol = require 'wez.dmux_bridge.protocol'
local json = require 'wez.dmux_bridge.json'
local consumer = require 'wez.dmux_bridge.consumer'

local host = '22222222-2222-4222-8222-222222222222'
local space = '33333333-3333-4333-8333-333333333333'
local epoch = '55555555-5555-4555-8555-555555555555'
local now = os.time()

local function request(uid, issued_at, expiry)
  local value = {
    protocol_version = 1,
    uid = uid,
    action = 'ping',
    target = {},
    issued_at = issued_at or now - 1,
    expiry = expiry or now + 9,
    nonce = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    replay_key = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
    origin = {
      kind = 'in_gui',
      gui_instance = state.id,
      pane_id = 91,
      domain = 'dmux-b-usb',
      host_uid = host,
      space_uid = space,
      server_epoch = epoch,
    },
  }
  assert(protocol.sign(value, state.key))
  return value
end

local function put(token, value)
  local path = requests .. '/req-' .. value.uid .. '.json'
  known_requests[path] = true
  local file = assert(io.open(path, 'wb'))
  assert(file:write(assert(json.encode(value))))
  file:close()
  return path
end

local function read_raw(path)
  return assert(fake_fs.read(path, 64 * 1024))
end

local function ack(path)
  return assert(json.decode(read_raw(path)))
end

local function poll_once()
  local callback = table.remove(scheduled, 1)
  assert(callback, 'poller failed to reschedule itself')
  callback()
end

local uid = '11111111-1111-4111-8111-111111111111'
local first = request(uid)
local request_path = put('REQUEST-ONE', first)
assert(consumer.start())
assert(dispatch_count == 1)
assert(consumer.start())
assert(#scheduled == 1, 'a second startup event must not create another poller')
assert(not io.open(request_path, 'rb'))
local primary_path = acks .. '/ack-' .. uid .. '.json'
local primary_bytes = read_raw(primary_path)
local primary = ack(primary_path)
assert(primary.ok == true and primary.pong == true and primary.request_sha256:match '^[0-9a-f]+$')

-- A resubmitted identical document never changes the byte-identical primary
-- acknowledgement and receives a distinct replay record.
put('REQUEST-ONE', first)
poll_once()
assert(dispatch_count == 1)
assert(read_raw(primary_path) == primary_bytes)
local replay_path = acks .. '/ack-' .. uid .. '.replay.json'
assert(ack(replay_path).error == 'replayed')

-- Consumed-without-ack resumes the idempotent presentation action once.
assert(os.remove(primary_path))
assert(os.remove(replay_path))
put('REQUEST-ONE', first)
poll_once()
assert(dispatch_count == 2)
assert(ack(primary_path).ok == true)

-- Reusing a consumed UID for different signed content cannot retarget it.
local conflict = clone(first)
conflict.replay_key = 'cccccccccccccccccccccccccccccccc'
assert(protocol.sign(conflict, state.key))
put('REQUEST-CONFLICT', conflict)
poll_once()
assert(dispatch_count == 2)
assert(ack(replay_path).error == 'request_uid_conflict')

-- An unexpected action callback error is converted to a typed ack and the
-- busy latch is released so the watchdog/poller continues.
local throwing = request '66666666-6666-4666-8666-666666666666'
throw_once = true
put('REQUEST-THROW', throwing)
poll_once()
assert(dispatch_count == 3)
assert(ack(acks .. '/ack-' .. throwing.uid .. '.json').error == 'bridge_internal')

-- An otherwise valid expired request retains its canonical digest, allowing
-- the Rust client to validate the typed error instead of reporting bad ack.
local expired = request('77777777-7777-4777-8777-777777777777', now - 20, now - 10)
put('REQUEST-EXPIRED', expired)
poll_once()
local expired_ack = ack(acks .. '/ack-' .. expired.uid .. '.json')
assert(expired_ack.error == 'expired')
assert(#expired_ack.request_sha256 == 64 and expired_ack.request_sha256:match '^[0-9a-f]+$')
assert(dispatch_count == 3)

-- Even an unauthenticated request cannot overwrite a pre-existing primary
-- acknowledgement whose consumed record is missing.
local planted = request '88888888-8888-4888-8888-888888888888'
planted.hmac_sha256 = string.rep('0', 64)
local planted_primary = acks .. '/ack-' .. planted.uid .. '.json'
local planted_file = assert(io.open(planted_primary, 'wb'))
assert(planted_file:write 'PLANTED-PRIMARY')
planted_file:close()
put('REQUEST-UNAUTHORIZED', planted)
poll_once()
assert(read_raw(planted_primary) == 'PLANTED-PRIMARY')
assert(ack(acks .. '/ack-' .. planted.uid .. '.replay.json').error == 'unauthorized')
assert(dispatch_count == 3)

assert(heartbeat_count >= 5)
assert(#scheduled == 1, 'exactly one watchdog callback must remain scheduled')

io.stdout:write 'dmux bridge consumer test: replay/resume/conflict/expiry passed\n'
