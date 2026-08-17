local context = require 'wez.dmux_bridge.context'
local instance = require 'wez.dmux_bridge.instance'
local json = require 'wez.dmux_bridge.json'
local wezterm = require 'wezterm'

local M = {}

local function binary()
  return os.getenv 'DMUX_BIN' or (wezterm.home_dir .. '/.local/bin/dmux')
end

local function toast(window, message)
  if window then
    pcall(function()
      window:toast_notification('dmux', message, nil, 5000)
    end)
  end
end

local function origin_for(pane)
  local marker, marker_err = context.from_pane(pane)
  if not marker then
    return nil, marker_err.message
  end
  local gui_instance = wezterm.GLOBAL.dmux_bridge_instance
  if type(gui_instance) ~= 'string' or #gui_instance == 0 then
    return nil, 'trusted dmux GUI bridge is not ready'
  end
  local origin = {
    protocol_version = 1,
    gui_instance = gui_instance,
    pane_id = marker.gui_pane_id,
    domain = marker.gui_domain,
    marker = context.marker_context(marker),
  }
  if marker.tmux_client_uid then
    origin.tmux_client_uid = marker.tmux_client_uid
  end
  return origin, marker
end

local function argv(origin, verb, args)
  local origin_json = assert(json.encode(origin))
  local out = { binary(), '_gui', '--origin-json', origin_json, verb }
  for _, arg in ipairs(args or {}) do
    table.insert(out, tostring(arg))
  end
  return out
end

local function decode_response(stdout)
  if type(stdout) ~= 'string' or #stdout > 64 * 1024 then
    return nil, 'controller response exceeds the bridge limit'
  end
  local response = json.decode(stdout)
  if type(response) ~= 'table' or response.schema_version ~= 1 or type(response.ok) ~= 'boolean' then
    return nil, 'controller returned a malformed response'
  end
  local allowed = { schema_version = true, ok = true, result = true, error = true, message = true }
  for key in pairs(response) do
    if type(key) ~= 'string' or not allowed[key] then
      return nil, 'controller returned a response with unknown fields'
    end
  end
  if response.ok then
    if
      response.error ~= nil
      or response.message ~= nil
      or (response.result ~= nil and type(response.result) ~= 'table')
    then
      return nil, 'controller returned a malformed success response'
    end
  elseif type(response.error) ~= 'string' or type(response.message) ~= 'string' then
    return nil, 'controller returned a malformed failure response'
  elseif response.error == 'partial_result' then
    if type(response.result) ~= 'table' then
      return nil, 'controller partial_result omitted its preserved result'
    end
  elseif response.result ~= nil then
    return nil, 'ordinary controller failure returned a result'
  end
  return response
end

local function invoke(window, origin, verb, args)
  local spawned, success, stdout, stderr = pcall(wezterm.run_child_process, argv(origin, verb, args))
  if not spawned then
    toast(window, 'dmux controller unavailable: ' .. tostring(success))
    return nil, tostring(success)
  end
  local response, decode_err = decode_response(stdout)
  if response then
    if not response.ok then
      toast(window, response.message)
      -- A post-create presentation failure preserves the owner-side
      -- CreatedSpace record. It remains an error (never a false success),
      -- but callers that can render richer recovery UX may inspect result.
      return nil, response.message, response.error == 'partial_result' and response.result or nil
    end
    if not success then
      local message = 'dmux controller returned success JSON with an unsuccessful exit status'
      toast(window, message)
      return nil, message
    end
    return response.result or {}
  end
  if not success then
    local message = tostring(stderr):gsub('%s+$', '')
    if #message == 0 then
      message = 'dmux controller exited unsuccessfully'
    end
    toast(window, message)
    return nil, message
  end
  toast(window, decode_err)
  return nil, decode_err
end

function M.run(window, pane, verb, args)
  local origin, marker_or_err = origin_for(pane)
  if not origin then
    toast(window, marker_or_err)
    return nil, marker_or_err
  end
  local result, err, partial = invoke(window, origin, verb, args)
  if not result then
    return nil, err, partial
  end
  return result, marker_or_err
end

---Issue the sole markerless GUI command from the exact active fork lease.
---A resident origin cannot be generalized to pane/Space mutations: it exists
---only so a zero-window application quit can enter the signed safe-quit flow.
function M.run_resident(verb, args)
  if verb ~= 'safe-quit' then
    return nil, 'resident GUI origin is restricted to safe-quit'
  end
  if args ~= nil and (type(args) ~= 'table' or next(args) ~= nil) then
    return nil, 'resident safe-quit does not accept arguments'
  end
  local identity, identity_err = instance.current_identity()
  if not identity then
    return nil, identity_err
  end
  local bridge, bridge_err = instance.current_bridge(identity.gui_instance)
  if not bridge then
    return nil, bridge_err
  end
  local brokered_ok, brokered = pcall(function()
    return bridge:resident_brokered()
  end)
  if not brokered_ok or brokered ~= true then
    return nil, 'trusted dmux GUI bridge lease is not broker-established'
  end
  return invoke(nil, {
    protocol_version = 1,
    kind = 'resident_gui',
    gui_instance = identity.gui_instance,
    pid = identity.pid,
    process_start_token = identity.process_start_token,
  }, verb, {})
end

function M.background_refresh(pane)
  local origin = origin_for(pane)
  if not origin then
    return false
  end
  local ok, err = pcall(wezterm.background_child_process, argv(origin, 'context', { '--cache' }))
  if not ok then
    wezterm.log_warn('dmux context refresh failed to start: ' .. tostring(err))
    return false
  end
  return true
end

local MARKER_FIELDS = {
  'host_uid',
  'space_uid',
  'space_no',
  'backend',
  'domain',
  'server_epoch',
  'group_ref',
  'split_ref',
}

local MARKER_KEYS = {
  backend = true,
  domain = true,
  group_ref = true,
  host_uid = true,
  server_epoch = true,
  space_no = true,
  space_uid = true,
  split_ref = true,
}

local CACHE_KEYS = {
  display = true,
  error = true,
  gui_instance = true,
  marker = true,
  message = true,
  ok = true,
  pane_id = true,
  schema_version = true,
  validated_at = true,
}

local DISPLAY_KEYS = {
  backend = true,
  group_count = true,
  group_name = true,
  logical_ref = true,
  owner_alias = true,
  owner_label = true,
  route = true,
  space_name = true,
  split_count = true,
}

local function exact_keys(value, allowed)
  if type(value) ~= 'table' then
    return false
  end
  for key in pairs(value) do
    if type(key) ~= 'string' or not allowed[key] then
      return false
    end
  end
  return true
end

local function integer(value, minimum)
  return type(value) == 'number' and value % 1 == 0 and value >= minimum and value <= 9007199254740991
end

local function bounded_string(value, maximum)
  return type(value) == 'string' and #value > 0 and #value <= maximum and not value:find '[%z\1-\31\127]'
end

local function marker_equal(left, right)
  if type(left) ~= 'table' or type(right) ~= 'table' then
    return false
  end
  for _, field in ipairs(MARKER_FIELDS) do
    if left[field] ~= right[field] then
      return false
    end
  end
  return true
end

function M.cached_context(pane, now)
  local origin, marker_or_err = origin_for(pane)
  if not origin then
    return nil, marker_or_err, 'invalid_marker'
  end
  local bridge, bridge_err = instance.current_bridge(origin.gui_instance)
  if not bridge then
    return nil, bridge_err, 'unverified'
  end
  local read_ok, raw = pcall(function()
    return bridge:read_context(origin.pane_id, 64 * 1024)
  end)
  if not read_ok then
    return nil, 'context read failed: ' .. tostring(raw), 'unverified'
  end
  if type(raw) ~= 'string' then
    return nil, 'context has not been validated yet', 'unverified'
  end
  local cache = json.decode(raw)
  if not exact_keys(cache, CACHE_KEYS) or cache.schema_version ~= 1 or type(cache.ok) ~= 'boolean' then
    return nil, 'validated context cache is malformed', 'invalid_cache'
  end
  now = now or os.time()
  if
    cache.gui_instance ~= origin.gui_instance
    or cache.pane_id ~= origin.pane_id
    or not integer(cache.pane_id, 0)
    or not integer(cache.validated_at, 0)
    or not exact_keys(cache.marker, MARKER_KEYS)
    or cache.validated_at < now - 2
    or cache.validated_at > now + 2
    or not marker_equal(cache.marker, origin.marker)
  then
    return nil, 'validated context cache is stale or belongs to another marker', 'unverified'
  end
  if cache.ok ~= true then
    if cache.display ~= nil or not bounded_string(cache.error, 128) or not bounded_string(cache.message, 4096) then
      return nil, 'validated context failure record is malformed', 'invalid_cache'
    end
    return nil, cache.message, 'invalid_context'
  end
  local display = cache.display
  if
    cache.error ~= nil
    or cache.message ~= nil
    or not exact_keys(display, DISPLAY_KEYS)
    or not bounded_string(display.logical_ref, 512)
    or not bounded_string(display.space_name, 1024)
    or (display.backend ~= 'wez' and display.backend ~= 'tmux')
    or not bounded_string(display.owner_alias, 64)
    or not bounded_string(display.owner_label, 128)
    or not bounded_string(display.route, 256)
    or not integer(display.group_count, 1)
    or not integer(display.split_count, 1)
    or (display.group_name ~= nil and not bounded_string(display.group_name, 1024))
  then
    return nil, 'validated context display record is malformed', 'invalid_cache'
  end
  return cache, marker_or_err
end

function M.toast(window, message)
  toast(window, message)
end

return M
