local canonical = require 'wez.dmux_bridge.canonical'
local crypto = require 'wez.dmux_bridge.crypto'
local instance = require 'wez.dmux_bridge.instance'
local json = require 'wez.dmux_bridge.json'
local presentation = require 'wez.dmux_bridge.presentation'
local protocol = require 'wez.dmux_bridge.protocol'
local wezterm = require 'wezterm'

local M = {}

local UUID = '^[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]$'

local ACK_KEYS = {
  action = true,
  already_hidden = true,
  completed_at = true,
  detached_domains = true,
  domain = true,
  domain_state = true,
  error = true,
  group_ref = true,
  gui_instance = true,
  message = true,
  nonce = true,
  ok = true,
  pane_id = true,
  platform_action = true,
  pong = true,
  protocol_version = true,
  reattached_domains = true,
  request_sha256 = true,
  resident_established = true,
  split_ref = true,
  toasted = true,
  uid = true,
  window_ids = true,
  workspace = true,
}

local function bridge_read(state, method, uid)
  local ok, value = pcall(function()
    return state.bridge[method](state.bridge, uid, protocol.MAX_DOCUMENT_BYTES)
  end)
  if not ok then
    return nil, tostring(value)
  end
  if value ~= nil and type(value) ~= 'string' then
    return nil, method .. ' returned a non-string document'
  end
  return value
end

local function discard_observed(state, uid)
  local ok, result = pcall(function()
    return state.bridge:discard_observed_request(uid)
  end)
  if not ok or result == false then
    return nil, tostring(result)
  end
  return true
end

local function ack_document(state, request, digest, result, err)
  local ack = {
    protocol_version = protocol.VERSION,
    uid = request.uid,
    action = request.action,
    nonce = request.nonce,
    ok = err == nil,
    completed_at = os.time(),
    request_sha256 = digest,
    gui_instance = state.id,
  }
  if err then
    ack.error = err.code
    ack.message = err.message
  end
  local after_ack
  if result then
    for key, value in pairs(result) do
      if key == 'after_ack' then
        after_ack = value
      else
        ack[key] = value
      end
    end
  end
  local body, encode_err = json.encode(ack)
  if not body then
    return nil, 'cannot encode acknowledgement: ' .. tostring(encode_err)
  end
  if #body > protocol.MAX_DOCUMENT_BYTES then
    return nil, 'acknowledgement exceeds the bridge document limit'
  end
  return body, after_ack
end

local function write_ack(state, replay, request, digest, result, err)
  local body, after_ack_or_err = ack_document(state, request, digest, result, err)
  if not body then
    return nil, after_ack_or_err
  end
  local method = replay and 'write_replay_ack_new' or 'write_ack_new'
  local ok, write_result = pcall(function()
    return state.bridge[method](state.bridge, request.uid, body)
  end)
  if not ok or write_result == false then
    return nil, tostring(write_result)
  end
  if after_ack_or_err then
    local callback_ok, callback_err = pcall(after_ack_or_err)
    if not callback_ok then
      wezterm.log_error('dmux bridge: post-ack action failed: ' .. tostring(callback_err))
    end
  end
  return true
end

local function stub_request(uid)
  return { uid = uid, action = 'unknown', nonce = '' }
end

local function exact_ack(raw, state, uid, prior, prior_digest)
  if #raw > protocol.MAX_DOCUMENT_BYTES then
    return nil, 'primary acknowledgement is oversized'
  end
  local ack = json.decode(raw)
  if type(ack) ~= 'table' then
    return nil, 'primary acknowledgement is not one JSON object'
  end
  for key in pairs(ack) do
    if type(key) ~= 'string' or not ACK_KEYS[key] then
      return nil, 'primary acknowledgement has an unknown field'
    end
  end
  if
    ack.protocol_version ~= protocol.VERSION
    or ack.uid ~= uid
    or ack.gui_instance ~= state.id
    or type(ack.action) ~= 'string'
    or type(ack.nonce) ~= 'string'
    or type(ack.ok) ~= 'boolean'
    or type(ack.completed_at) ~= 'number'
    or ack.completed_at % 1 ~= 0
    or type(ack.request_sha256) ~= 'string'
    or #ack.request_sha256 ~= 64
    or not ack.request_sha256:match '^[0-9a-f]+$'
  then
    return nil, 'primary acknowledgement common fields are malformed'
  end
  if prior and (ack.action ~= prior.action or ack.nonce ~= prior.nonce or ack.request_sha256 ~= prior_digest) then
    return nil, 'primary acknowledgement differs from its consumed request'
  end
  return ack
end

local function exact_consumed(raw, state, uid)
  if #raw > protocol.MAX_DOCUMENT_BYTES then
    return nil, nil, 'consumed request is oversized'
  end
  local request = json.decode(raw)
  if type(request) ~= 'table' or request.uid ~= uid then
    return nil, nil, 'consumed request is malformed or has the wrong UID'
  end
  -- Validate at its own issue instant: this proves its exact schema,
  -- canonical signing document and HMAC without reviving it after expiry.
  local authenticated, auth_err, digest =
    protocol.validate_and_authenticate(request, state.key, request.issued_at, state.id)
  if not authenticated then
    return nil, nil, 'consumed request authentication failed: ' .. tostring(auth_err and auth_err.code)
  end
  local document = canonical.signing_document(request)
  if not document or crypto.sha256(document) ~= digest then
    return nil, nil, 'consumed request canonical digest is inconsistent'
  end
  return request, digest
end

local function fatal_corruption(state, uid, request, digest, message, primary_is_durable)
  request = request or stub_request(uid)
  digest = digest or string.rep('0', 64)
  local durable = primary_is_durable
  if not durable then
    local replay_ok, replay_err = write_ack(state, true, request, digest, nil, {
      code = 'replay_corruption',
      message = message,
    })
    durable = replay_ok
    if not replay_ok then
      wezterm.log_error('dmux bridge: cannot publish replay corruption ack: ' .. tostring(replay_err))
    end
  end
  if durable then
    local discarded, discard_err = discard_observed(state, uid)
    if not discarded then
      wezterm.log_error('dmux bridge: cannot discard corrupt replay request: ' .. tostring(discard_err))
    end
  end
  state.failed = true
  wezterm.log_error('dmux bridge: fatal persisted replay corruption: ' .. message)
end

local function publish_replay_and_discard(state, request, digest, code, message, primary_is_durable)
  local durable = primary_is_durable
  if not durable then
    local ok, err = write_ack(state, true, request, digest or string.rep('0', 64), nil, {
      code = code,
      message = message,
    })
    durable = ok
    if not ok then
      wezterm.log_error('dmux bridge: cannot publish replay acknowledgement: ' .. tostring(err))
    end
  else
    -- A replay record is observability only; the immutable primary already
    -- makes discarding this exact duplicate durable and safe.
    local ok, err = write_ack(state, true, request, digest or string.rep('0', 64), nil, {
      code = code,
      message = message,
    })
    if not ok then
      wezterm.log_error('dmux bridge: replay record already exists or cannot be written: ' .. tostring(err))
    end
  end
  if durable then
    local discarded, discard_err = discard_observed(state, request.uid)
    if not discarded then
      state.failed = true
      wezterm.log_error('dmux bridge: cannot discard observed replay: ' .. tostring(discard_err))
    end
  end
end

local function consume_new(state, uid)
  local ok, result = pcall(function()
    return state.bridge:consume_request_new(uid)
  end)
  if not ok or result == false then
    return nil, tostring(result)
  end
  return true
end

local function process_request(state, uid, raw)
  local prior_raw, prior_read_err = bridge_read(state, 'read_consumed', uid)
  if prior_read_err then
    fatal_corruption(state, uid, nil, nil, 'cannot read consumed evidence: ' .. prior_read_err, false)
    return
  end
  local primary_raw, primary_read_err = bridge_read(state, 'read_ack', uid)
  if primary_read_err then
    fatal_corruption(state, uid, nil, nil, 'cannot read primary acknowledgement: ' .. primary_read_err, false)
    return
  end

  local prior, prior_digest
  if prior_raw then
    local prior_err
    prior, prior_digest, prior_err = exact_consumed(prior_raw, state, uid)
    if not prior then
      fatal_corruption(state, uid, nil, nil, prior_err, false)
      return
    end
  end
  local primary
  if primary_raw then
    local primary_err
    primary, primary_err = exact_ack(primary_raw, state, uid, prior, prior_digest)
    if not primary then
      fatal_corruption(state, uid, prior, prior_digest, primary_err, false)
      return
    end
  end

  local request
  local parse_error
  if type(raw) ~= 'string' then
    parse_error = 'malformed_request'
  elseif #raw > protocol.MAX_DOCUMENT_BYTES then
    parse_error = 'message_too_large'
  else
    request = json.decode(raw)
    if type(request) ~= 'table' or request.uid ~= uid then
      request = nil
      parse_error = 'malformed_request'
    end
  end
  if not request then
    local stub = stub_request(uid)
    if prior or primary then
      publish_replay_and_discard(
        state,
        stub,
        nil,
        'request_uid_conflict',
        'pending request cannot be matched to persisted evidence',
        primary ~= nil
      )
    else
      local consumed, consume_err = consume_new(state, uid)
      if not consumed then
        state.failed = true
        wezterm.log_error('dmux bridge: cannot consume malformed request: ' .. tostring(consume_err))
        return
      end
      local ok, ack_err = write_ack(state, false, stub, string.rep('0', 64), nil, {
        code = parse_error,
        message = 'request is not one bounded JSON object',
      })
      if not ok then
        state.failed = true
        wezterm.log_error('dmux bridge: cannot acknowledge malformed request: ' .. tostring(ack_err))
      end
    end
    return
  end

  local authenticated, auth_err, digest = protocol.validate_and_authenticate(request, state.key, os.time(), state.id)
  if not authenticated then
    if prior then
      if digest and digest == prior_digest and not primary then
        local ok, ack_err = write_ack(state, false, request, digest, nil, auth_err)
        if not ok then
          state.failed = true
          wezterm.log_error('dmux bridge: cannot finish consumed rejection: ' .. tostring(ack_err))
          return
        end
        local discarded, discard_err = discard_observed(state, uid)
        if not discarded then
          state.failed = true
          wezterm.log_error('dmux bridge: cannot discard rejected retry: ' .. tostring(discard_err))
        end
      else
        publish_replay_and_discard(
          state,
          request,
          digest,
          digest == prior_digest and auth_err.code or 'request_uid_conflict',
          digest == prior_digest and auth_err.message or 'request UID was reused with different content',
          primary ~= nil
        )
      end
    elseif primary then
      publish_replay_and_discard(
        state,
        request,
        digest,
        'request_uid_conflict',
        'primary acknowledgement exists without matching consumed evidence',
        true
      )
    else
      local consumed, consume_err = consume_new(state, uid)
      if not consumed then
        state.failed = true
        wezterm.log_error('dmux bridge: cannot consume rejected request: ' .. tostring(consume_err))
        return
      end
      local ok, ack_err = write_ack(state, false, request, digest or string.rep('0', 64), nil, auth_err)
      if not ok then
        state.failed = true
        wezterm.log_error('dmux bridge: cannot acknowledge rejected request: ' .. tostring(ack_err))
      end
    end
    return
  end

  if prior then
    if prior_digest ~= digest then
      publish_replay_and_discard(
        state,
        request,
        digest,
        'request_uid_conflict',
        'request UID was reused with different content',
        primary ~= nil
      )
      return
    end
    if primary then
      publish_replay_and_discard(
        state,
        request,
        digest,
        'replayed',
        'request UID was already consumed; read the original ack',
        true
      )
      return
    end
    local discarded, discard_err = discard_observed(state, uid)
    if not discarded then
      state.failed = true
      wezterm.log_error('dmux bridge: cannot discard crash-recovery retry: ' .. tostring(discard_err))
      return
    end
  else
    if primary then
      publish_replay_and_discard(
        state,
        request,
        digest,
        'request_uid_conflict',
        'ack exists without the matching consumed request',
        true
      )
      return
    end
    local consumed, consume_err = consume_new(state, uid)
    if not consumed then
      state.failed = true
      wezterm.log_error('dmux bridge: cannot consume request atomically: ' .. tostring(consume_err))
      return
    end
  end

  state.busy = true
  local function complete(result, dispatch_err)
    local ok, callback_err = pcall(function()
      local written, write_err = write_ack(state, false, request, digest, result, dispatch_err)
      if not written then
        state.failed = true
        wezterm.log_error('dmux bridge: cannot write completion ack: ' .. tostring(write_err))
      end
    end)
    if not ok then
      state.failed = true
      wezterm.log_error('dmux bridge: completion callback failed: ' .. tostring(callback_err))
    end
    state.busy = false
  end
  local dispatched, dispatch_err = pcall(presentation.dispatch, request, state, complete)
  if not dispatched then
    complete(nil, { code = 'bridge_internal', message = tostring(dispatch_err) })
  end
end

local function poll(state)
  local ok, err = pcall(function()
    local heartbeat_ok, heartbeat_err = instance.heartbeat(state)
    if not heartbeat_ok then
      state.failed = true
      wezterm.log_error('dmux bridge: heartbeat failed closed: ' .. tostring(heartbeat_err))
    end
    if not state.busy and not state.failed then
      local next_ok, uid, raw = pcall(function()
        return state.bridge:next_request(protocol.MAX_DOCUMENT_BYTES)
      end)
      if not next_ok then
        state.failed = true
        wezterm.log_error('dmux bridge: secure request read failed closed: ' .. tostring(uid))
      elseif uid ~= nil or raw ~= nil then
        if type(uid) ~= 'string' or not uid:match(UUID) or type(raw) ~= 'string' then
          state.failed = true
          wezterm.log_error 'dmux bridge: secure request reader returned a malformed tuple'
        else
          process_request(state, uid, raw)
        end
      end
    end
  end)
  if not ok then
    state.failed = true
    wezterm.log_error('dmux bridge: poll failed closed: ' .. tostring(err))
  end
  wezterm.time.call_after(protocol.POLL_SECONDS, function()
    poll(state)
  end)
end

function M.start()
  if wezterm.GLOBAL.dmux_bridge_poller_started or wezterm.GLOBAL.dmux_bridge_poller_starting then
    return true
  end
  wezterm.GLOBAL.dmux_bridge_poller_starting = true
  local created, state, err = pcall(instance.create)
  wezterm.GLOBAL.dmux_bridge_poller_starting = false
  if not created or not state then
    local detail = created and err or state
    wezterm.log_error('dmux bridge disabled: ' .. tostring(detail))
    return nil, detail
  end
  if type(state.bridge) ~= 'userdata' and type(state.bridge) ~= 'table' then
    wezterm.log_error 'dmux bridge disabled: maintained-fork lease is unavailable'
    return nil, 'maintained-fork lease is unavailable'
  end
  wezterm.GLOBAL.dmux_bridge_poller_started = true
  wezterm.GLOBAL.dmux_bridge_instance = state.id
  poll(state)
  return true
end

return M
