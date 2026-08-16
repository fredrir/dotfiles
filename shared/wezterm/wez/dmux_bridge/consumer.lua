local canonical = require 'wez.dmux_bridge.canonical'
local crypto = require 'wez.dmux_bridge.crypto'
local fs = require 'wez.dmux_bridge.fs'
local instance = require 'wez.dmux_bridge.instance'
local json = require 'wez.dmux_bridge.json'
local presentation = require 'wez.dmux_bridge.presentation'
local protocol = require 'wez.dmux_bridge.protocol'
local wezterm = require 'wezterm'

local M = {}

local function read_json(path, maximum)
  local raw, read_err = fs.read(path, maximum)
  if not raw then
    return nil, read_err
  end
  local value = json.decode(raw)
  if type(value) ~= 'table' then
    return nil, 'malformed_json'
  end
  return value, nil, raw
end

local function uuid_from_request_path(path)
  local base = path:match '([^/]+)$'
  local uid = base and base:match '^req%-([0-9a-f%-]+)%.json$'
  if not uid or #uid ~= 36 then
    return nil
  end
  return uid
end

local function ack_path(state, uid)
  return fs.join(state.paths.acks, 'ack-' .. uid .. '.json')
end

local function consumed_path(state, uid)
  return fs.join(state.paths.consumed, 'req-' .. uid .. '.json')
end

local function write_ack_at(state, path, request, digest, result, err)
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
    wezterm.log_error('dmux bridge: cannot encode ack: ' .. tostring(encode_err))
    return nil
  end
  if #body > protocol.MAX_DOCUMENT_BYTES then
    wezterm.log_error 'dmux bridge: refusing oversized acknowledgement'
    return nil
  end
  local ok, write_err = fs.write_private_atomic(path, body)
  if not ok then
    wezterm.log_error('dmux bridge: cannot write ack ' .. path .. ': ' .. tostring(write_err))
    return nil
  end
  if after_ack then
    -- The acknowledgement rename is complete before the lifecycle action.
    -- Execute in the same callback to minimize the only remaining crash
    -- window; Hide/Quit is never attempted before a verifiable ack exists.
    local callback_ok, callback_err = pcall(after_ack)
    if not callback_ok then
      wezterm.log_error('dmux bridge: post-ack action failed: ' .. tostring(callback_err))
    end
  end
  return true
end

local function malformed_request(state, path, uid, code, message)
  local request = {
    uid = uid,
    action = 'unknown',
    nonce = '',
  }
  if
    fs.read(consumed_path(state, uid), protocol.MAX_DOCUMENT_BYTES)
    or fs.read(ack_path(state, uid), protocol.MAX_DOCUMENT_BYTES)
  then
    os.remove(path)
    write_ack_at(
      state,
      fs.join(state.paths.acks, 'ack-' .. uid .. '.replay.json'),
      request,
      '',
      nil,
      { code = code, message = message }
    )
  else
    os.rename(path, consumed_path(state, uid))
    write_ack_at(state, ack_path(state, uid), request, '', nil, { code = code, message = message })
  end
end

local function replay_ack(state, request, digest, code, message)
  local path = fs.join(state.paths.acks, string.format('ack-%s.replay.json', request.uid))
  write_ack_at(state, path, request, digest, nil, { code = code, message = message })
end

local function consumed_digest(state, uid)
  local prior = read_json(consumed_path(state, uid), protocol.MAX_DOCUMENT_BYTES)
  if not prior then
    return nil
  end
  local document = canonical.signing_document(prior)
  return document and crypto.sha256(document) or nil
end

local function process_request(state, path, uid)
  local request, parse_err = read_json(path, protocol.MAX_DOCUMENT_BYTES)
  if not request then
    malformed_request(
      state,
      path,
      uid,
      parse_err == 'too_large' and 'message_too_large' or 'malformed_request',
      'request is not one bounded JSON object'
    )
    return
  end
  if request.uid ~= uid then
    malformed_request(state, path, uid, 'malformed_request', 'filename UID differs from request.uid')
    return
  end

  local authenticated, auth_err, digest = protocol.validate_and_authenticate(request, state.key, os.time(), state.id)
  if not authenticated then
    local already_consumed = fs.read(consumed_path(state, uid), protocol.MAX_DOCUMENT_BYTES) ~= nil
    local primary_exists = fs.read(ack_path(state, uid), protocol.MAX_DOCUMENT_BYTES) ~= nil
    if already_consumed or primary_exists then
      os.remove(path)
      replay_ack(state, request, digest or '', auth_err.code, auth_err.message)
    else
      os.rename(path, consumed_path(state, uid))
      write_ack_at(state, ack_path(state, uid), request, digest or '', nil, auth_err)
    end
    return
  end

  local prior_digest = consumed_digest(state, uid)
  if prior_digest then
    os.remove(path)
    if prior_digest == digest then
      if fs.read(ack_path(state, uid), protocol.MAX_DOCUMENT_BYTES) then
        -- The ordinary client retry is an idempotent re-read of the original
        -- ack. A re-submitted request is separately observable as replay,
        -- without ever changing the byte-identical original ack.
        replay_ack(state, request, digest, 'replayed', 'request UID was already consumed; read the original ack')
        return
      end
      -- Crash recovery: the request was durably consumed before its ack.
      -- Every bridge action is presentation-only and idempotent, so resume.
    else
      replay_ack(state, request, digest, 'request_uid_conflict', 'request UID was reused with different content')
      return
    end
  else
    if fs.read(ack_path(state, uid), protocol.MAX_DOCUMENT_BYTES) then
      os.remove(path)
      replay_ack(state, request, digest, 'request_uid_conflict', 'ack exists without the matching consumed request')
      return
    end
    local moved, move_err = os.rename(path, consumed_path(state, uid))
    if not moved then
      write_ack_at(state, ack_path(state, uid), request, digest, nil, {
        code = 'bridge_internal',
        message = 'cannot consume request atomically: ' .. tostring(move_err),
      })
      return
    end
  end

  state.busy = true
  local function complete(result, dispatch_err)
    local ok, callback_err = pcall(function()
      write_ack_at(state, ack_path(state, uid), request, digest, result, dispatch_err)
    end)
    if not ok then
      wezterm.log_error('dmux bridge: completion callback failed: ' .. tostring(callback_err))
    end
    state.busy = false
  end
  local dispatched, dispatch_err = pcall(presentation.dispatch, request, state, complete)
  if not dispatched then
    complete(nil, { code = 'bridge_internal', message = tostring(dispatch_err) })
  end
end

local function next_request(state)
  local entries = wezterm.read_dir(state.paths.requests)
  table.sort(entries)
  for _, path in ipairs(entries) do
    local uid = uuid_from_request_path(path)
    if uid then
      return path, uid
    end
  end
  return nil
end

local function poll(state)
  local ok, err = pcall(function()
    local heartbeat_ok, heartbeat_err = instance.heartbeat(state)
    if not heartbeat_ok then
      wezterm.log_error('dmux bridge: heartbeat failed: ' .. tostring(heartbeat_err))
    end
    if not state.busy then
      local path, uid = next_request(state)
      if path then
        process_request(state, path, uid)
      end
    end
  end)
  if not ok then
    wezterm.log_error('dmux bridge: poll failed: ' .. tostring(err))
  end
  wezterm.time.call_after(protocol.POLL_SECONDS, function()
    local callback_ok, callback_err = pcall(poll, state)
    if not callback_ok then
      wezterm.log_error('dmux bridge: poll callback failed: ' .. tostring(callback_err))
      -- One last independent reschedule keeps an unexpected callback error
      -- from silently killing liveness forever.
      wezterm.time.call_after(protocol.POLL_SECONDS, function()
        poll(state)
      end)
    end
  end)
end

function M.start()
  if wezterm.GLOBAL.dmux_bridge_poller_started then
    return true
  end
  -- gui-startup and gui-attached may both fire while the async filesystem
  -- calls in instance.create() are yielded. Claim startup before that first
  -- yield so one GUI process cannot accidentally register two consumers.
  if wezterm.GLOBAL.dmux_bridge_poller_starting then
    return true
  end
  wezterm.GLOBAL.dmux_bridge_poller_starting = true
  local created, state, err = pcall(instance.create)
  wezterm.GLOBAL.dmux_bridge_poller_starting = false
  if not created then
    wezterm.log_error('dmux bridge disabled: ' .. tostring(state))
    return nil, state
  end
  if not state then
    wezterm.log_error('dmux bridge disabled: ' .. tostring(err))
    return nil, err
  end
  wezterm.GLOBAL.dmux_bridge_poller_started = true
  wezterm.GLOBAL.dmux_bridge_instance = state.id
  poll(state)
  return true
end

return M
