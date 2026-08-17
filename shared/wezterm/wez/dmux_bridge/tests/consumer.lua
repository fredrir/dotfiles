package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local fake_wezterm = { GLOBAL = {} }
package.preload.wezterm = function()
  return fake_wezterm
end

local json = require 'wez.dmux_bridge.json'
local protocol = require 'wez.dmux_bridge.protocol'

local host = '22222222-2222-4222-8222-222222222222'
local space = '33333333-3333-4333-8333-333333333333'
local epoch = '55555555-5555-4555-8555-555555555555'
local group = 'g' .. epoch .. '.wz-7'
local split = 'p' .. epoch .. '.wz-9'
local key = '0123456789abcdef0123456789abcdef'
local now = os.time()

local function clone(value)
  if type(value) ~= 'table' then
    return value
  end
  local out = {}
  for name, item in pairs(value) do
    out[name] = clone(item)
  end
  return out
end

local function signed_request(uid, issued_at, expiry)
  local request = {
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
  assert(protocol.sign(request, key))
  return request
end

local function new_bridge()
  local bridge = {
    pending = {},
    order = {},
    observed = {},
    consumed = {},
    primary = {},
    replay = {},
  }

  function bridge:enqueue(request_or_raw, uid)
    local raw = type(request_or_raw) == 'string' and request_or_raw or assert(json.encode(request_or_raw))
    uid = uid or request_or_raw.uid
    if self.pending[uid] == nil then
      table.insert(self.order, uid)
    end
    self.pending[uid] = raw
  end

  function bridge:next_request(maximum)
    for _, uid in ipairs(self.order) do
      local raw = self.pending[uid]
      if raw ~= nil then
        self.observed[uid] = raw
        if self.change_after_observe then
          self.pending[uid] = raw .. 'changed'
          self.change_after_observe = false
        end
        if #raw > maximum then
          return uid, raw:sub(1, maximum + 1)
        end
        return uid, raw
      end
    end
    return nil, nil
  end

  function bridge:read_consumed(uid)
    return self.consumed[uid]
  end

  function bridge:consume_request_new(uid)
    if self.pending[uid] == nil or self.observed[uid] ~= self.pending[uid] then
      error 'dmux_bridge_request_changed'
    end
    if self.consumed[uid] ~= nil then
      error 'dmux_bridge_consumed_exists'
    end
    self.consumed[uid] = self.pending[uid]
    self.pending[uid] = nil
    return true
  end

  function bridge:discard_observed_request(uid)
    if self.pending[uid] == nil or self.observed[uid] ~= self.pending[uid] then
      error 'dmux_bridge_request_changed'
    end
    self.pending[uid] = nil
    return true
  end

  function bridge:read_ack(uid)
    return self.primary[uid]
  end

  function bridge:write_ack_new(uid, body)
    if self.fail_primary_write then
      error 'dmux_bridge_ack_write_failed'
    end
    if self.primary[uid] ~= nil then
      error 'dmux_bridge_ack_exists'
    end
    self.primary[uid] = body
    return true
  end

  function bridge:write_replay_ack_new(uid, body)
    if self.replay[uid] ~= nil then
      error 'dmux_bridge_replay_ack_exists'
    end
    self.replay[uid] = body
    return true
  end

  return bridge
end

local function new_harness()
  local harness = {
    bridge = new_bridge(),
    scheduled = {},
    errors = {},
    dispatch_count = 0,
    heartbeat_count = 0,
    throw_once = false,
  }
  harness.state = {
    id = 'gui-42-cafe',
    pid = 42,
    process_start_token = 'start-token',
    key = key,
    bridge = harness.bridge,
    safe_quit = {},
  }

  fake_wezterm.GLOBAL = {}
  fake_wezterm.log_error = function(message)
    table.insert(harness.errors, message)
  end
  fake_wezterm.time = {
    call_after = function(_, callback)
      table.insert(harness.scheduled, callback)
    end,
  }
  package.loaded['wez.dmux_bridge.instance'] = {
    create = function()
      return harness.state
    end,
    heartbeat = function()
      harness.heartbeat_count = harness.heartbeat_count + 1
      return true
    end,
  }
  package.loaded['wez.dmux_bridge.presentation'] = {
    dispatch = function(request, _, done)
      harness.dispatch_count = harness.dispatch_count + 1
      if harness.throw_once then
        harness.throw_once = false
        error 'deterministic dispatch failure'
      end
      assert(request.action == 'ping')
      done { pong = true }
    end,
  }
  package.loaded['wez.dmux_bridge.consumer'] = nil
  harness.consumer = require 'wez.dmux_bridge.consumer'

  function harness:poll_once()
    local callback = table.remove(self.scheduled, 1)
    assert(callback, 'poller failed to reschedule itself')
    callback()
  end

  function harness:ack(uid, replay)
    local raw = replay and self.bridge.replay[uid] or self.bridge.primary[uid]
    return raw and assert(json.decode(raw))
  end

  return harness
end

local uid = '11111111-1111-4111-8111-111111111111'
local first = signed_request(uid)
local core = new_harness()
core.bridge:enqueue(first)
assert(core.consumer.start())
assert(core.dispatch_count == 1 and core.bridge.pending[uid] == nil)
local first_primary = assert(core.bridge.primary[uid])
assert(core:ack(uid).ok == true and core:ack(uid).pong == true)
assert(core.consumer.start() and #core.scheduled == 1, 'second start created another poller')

-- A byte-identical replay cannot overwrite the immutable primary and is
-- discarded only after durable replay evidence exists.
core.bridge:enqueue(first)
core:poll_once()
assert(core.dispatch_count == 1)
assert(core.bridge.primary[uid] == first_primary)
assert(core:ack(uid, true).error == 'replayed')
assert(core.bridge.pending[uid] == nil)

-- Consumed-without-ack crash recovery resumes the same authenticated action;
-- a different digest under that UID remains a conflict.
core.bridge.primary[uid] = nil
core.bridge.replay[uid] = nil
core.bridge:enqueue(first)
core:poll_once()
assert(core.dispatch_count == 2 and core:ack(uid).ok == true)
local conflict = clone(first)
conflict.replay_key = 'cccccccccccccccccccccccccccccccc'
assert(protocol.sign(conflict, key))
core.bridge:enqueue(conflict)
core:poll_once()
assert(core.dispatch_count == 2 and core:ack(uid, true).error == 'request_uid_conflict')

-- Callback exceptions become one typed primary ack and release the busy
-- latch. Expiry is tested at the equality boundary before any dispatch.
local throwing = signed_request '66666666-6666-4666-8666-666666666666'
core.throw_once = true
core.bridge:enqueue(throwing)
core:poll_once()
assert(core.dispatch_count == 3 and core:ack(throwing.uid).error == 'bridge_internal')
local expired = signed_request('77777777-7777-4777-8777-777777777777', now - 5, now)
core.bridge:enqueue(expired)
core:poll_once()
assert(core:ack(expired.uid).error == 'expired' and core.dispatch_count == 3)
assert(#core.scheduled == 1 and core.heartbeat_count >= 5)

-- Existing corrupt consumed evidence is a fatal bridge condition. It is
-- never overwritten or redispatched; only the separately observed pending
-- duplicate may be discarded after a durable replay-corruption ack.
local corrupt_uid = '88888888-8888-4888-8888-888888888888'
local corrupt = new_harness()
corrupt.bridge.consumed[corrupt_uid] = '{corrupt'
corrupt.bridge:enqueue(signed_request(corrupt_uid))
assert(corrupt.consumer.start())
assert(corrupt.state.failed == true and corrupt.dispatch_count == 0)
assert(corrupt.bridge.consumed[corrupt_uid] == '{corrupt')
assert(corrupt.bridge.pending[corrupt_uid] == nil)
assert(corrupt:ack(corrupt_uid, true).error == 'replay_corruption')

local oversized_uid = '99999999-9999-4999-8999-999999999999'
local oversized = new_harness()
oversized.bridge.consumed[oversized_uid] = string.rep('x', protocol.MAX_DOCUMENT_BYTES + 1)
oversized.bridge:enqueue(signed_request(oversized_uid))
assert(oversized.consumer.start())
assert(oversized.state.failed == true and oversized.dispatch_count == 0)
assert(#oversized.bridge.consumed[oversized_uid] == protocol.MAX_DOCUMENT_BYTES + 1)

local primary_uid = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
local corrupt_primary = new_harness()
corrupt_primary.bridge.primary[primary_uid] = 'PLANTED-PRIMARY'
corrupt_primary.bridge:enqueue(signed_request(primary_uid))
assert(corrupt_primary.consumer.start())
assert(corrupt_primary.state.failed == true and corrupt_primary.dispatch_count == 0)
assert(corrupt_primary.bridge.primary[primary_uid] == 'PLANTED-PRIMARY')
assert(corrupt_primary.bridge.pending[primary_uid] == nil)

-- Fork CAS failures fail closed before presentation; an ack publication
-- failure after dispatch permanently stops the poller so it cannot retry the
-- side effect without durable completion evidence.
local changed_uid = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
local changed = new_harness()
changed.bridge.change_after_observe = true
changed.bridge:enqueue(signed_request(changed_uid))
assert(changed.consumer.start())
assert(changed.state.failed == true and changed.dispatch_count == 0)

local ack_fail_uid = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc'
local ack_fail = new_harness()
ack_fail.bridge.fail_primary_write = true
ack_fail.bridge:enqueue(signed_request(ack_fail_uid))
assert(ack_fail.consumer.start())
assert(ack_fail.state.failed == true and ack_fail.dispatch_count == 1)
ack_fail:poll_once()
assert(ack_fail.dispatch_count == 1, 'failed completion was redispatched')

-- A latched failure is re-evaluated every poll. It must report the cause and
-- the consequence once, then at an interval, never once per POLL_SECONDS.
local repeat_ticks = math.max(1, math.floor(60 / protocol.POLL_SECONDS))
local flaky = new_harness()
local instance_module = package.loaded['wez.dmux_bridge.instance']
instance_module.heartbeat = function()
  return nil, 'heartbeat is broken'
end
assert(flaky.consumer.start())
assert(flaky.state.failed == true)
assert(#flaky.errors == 2, 'a latched failure must name its cause and its consequence')
assert(flaky.errors[1]:match 'heartbeat failed closed: heartbeat is broken')
assert(flaky.errors[2]:match 'latched failed; no further requests will be read')

for _ = 1, repeat_ticks - 1 do
  flaky:poll_once()
end
assert(#flaky.errors == 2, 'a latched failure logged before its interval elapsed')
flaky:poll_once()
assert(#flaky.errors == 4, 'a latched failure never repeated at its interval')
assert(flaky.errors[3]:match(string.format('%%(repeated %d times%%)$', repeat_ticks)))
assert(flaky.errors[4]:match(string.format('%%(repeated %d times%%)$', repeat_ticks)))

-- A changed message is a different failure and is never absorbed by the
-- interval the previous one left behind.
instance_module.heartbeat = function()
  return nil, 'heartbeat broke differently'
end
flaky:poll_once()
assert(#flaky.errors == 5 and flaky.errors[5]:match 'heartbeat broke differently')

-- A cleared condition is forgotten, so its next occurrence is immediate.
instance_module.heartbeat = function()
  return true
end
flaky:poll_once()
assert(#flaky.errors == 5, 'a recovered heartbeat is not a failure and must not log')
instance_module.heartbeat = function()
  return nil, 'heartbeat broke differently'
end
flaky:poll_once()
assert(#flaky.errors == 6, 'a recurrence after recovery must log immediately')

io.stdout:write 'dmux bridge consumer test: secure CAS/replay/corruption/expiry passed\n'
