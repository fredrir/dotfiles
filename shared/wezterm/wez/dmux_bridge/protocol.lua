local canonical = require 'wez.dmux_bridge.canonical'
local crypto = require 'wez.dmux_bridge.crypto'

local M = {
  VERSION = 1,
  MAX_DOCUMENT_BYTES = 64 * 1024,
  MAX_TTL_SECONDS = 10,
  POLL_SECONDS = 0.05,
  HEARTBEAT_STALE_SECONDS = 1,
}

M.ACTIONS = {
  activate = true,
  attach_domain = true,
  detach_domain = true,
  establish_resident = true,
  focus_pane = true,
  ping = true,
  present = true,
  safe_quit = true,
  toast = true,
}

local TOP_KEYS = {
  action = true,
  expiry = true,
  hmac_sha256 = true,
  issued_at = true,
  nonce = true,
  origin = true,
  protocol_version = true,
  replay_key = true,
  target = true,
  uid = true,
}

local TARGET_KEYS = {
  alternate_domains = true,
  backend_instance_uid = true,
  domain = true,
  domains = true,
  backend = true,
  group_ref = true,
  host_uid = true,
  message = true,
  pane_id = true,
  phase = true,
  platform_action = true,
  proof_uid = true,
  server_epoch = true,
  space_uid = true,
  split_ref = true,
  space_no = true,
  tmux_client_uid = true,
  workspace = true,
}

local TARGET_KEYS_BY_ACTION = {
  establish_resident = {
    backend_instance_uid = true,
    domain = true,
    host_uid = true,
    server_epoch = true,
    space_uid = true,
  },
  ping = {},
  toast = { message = true },
  attach_domain = {
    alternate_domains = true,
    backend_instance_uid = true,
    domain = true,
    server_epoch = true,
  },
  detach_domain = {
    backend_instance_uid = true,
    domain = true,
    server_epoch = true,
  },
  focus_pane = {
    backend = true,
    backend_instance_uid = true,
    domain = true,
    group_ref = true,
    host_uid = true,
    pane_id = true,
    server_epoch = true,
    space_no = true,
    space_uid = true,
    split_ref = true,
    tmux_client_uid = true,
  },
  activate = {
    backend_instance_uid = true,
    domain = true,
    group_ref = true,
    host_uid = true,
    server_epoch = true,
    space_uid = true,
    split_ref = true,
    workspace = true,
  },
  present = {
    alternate_domains = true,
    backend_instance_uid = true,
    domain = true,
    group_ref = true,
    host_uid = true,
    server_epoch = true,
    space_uid = true,
    split_ref = true,
    workspace = true,
  },
  safe_quit = {
    domains = true,
    phase = true,
    platform_action = true,
    proof_uid = true,
  },
}

local IN_GUI_KEYS = {
  backend = true,
  domain = true,
  group_ref = true,
  gui_instance = true,
  host_uid = true,
  kind = true,
  pane_id = true,
  pid = true,
  process_start_token = true,
  server_epoch = true,
  space_no = true,
  space_uid = true,
  split_ref = true,
  tmux_client_uid = true,
}

local COLD_KEYS = {
  backend_instance_uid = true,
  domain = true,
  gui_instance = true,
  host_uid = true,
  kind = true,
  launcher_request_uid = true,
  pid = true,
  server_epoch = true,
  space_uid = true,
  start_token = true,
  uid = true,
}

local RESIDENT_KEYS = {
  gui_instance = true,
  kind = true,
  pid = true,
  process_start_token = true,
}

local UUID = '^[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]$'

local function failure(code, message, digest)
  return nil, { code = code, message = message }, digest
end

local function exact_keys(value, allowed, label)
  if type(value) ~= 'table' then
    return nil, label .. ' must be an object'
  end
  for key in pairs(value) do
    if type(key) ~= 'string' or not allowed[key] then
      return nil, label .. ' has unknown field ' .. tostring(key)
    end
  end
  return true
end

local function string_field(value, label, maximum)
  if type(value) ~= 'string' or #value == 0 or #value > maximum then
    return nil, string.format('%s must be a non-empty string of at most %d bytes', label, maximum)
  end
  return true
end

local function uuid_field(value, label)
  if type(value) ~= 'string' or not value:match(UUID) then
    return nil, label .. ' must be a canonical lowercase UUID'
  end
  return true
end

local function uint_field(value, label)
  if type(value) ~= 'number' or value % 1 ~= 0 or value < 0 or value > 9007199254740991 then
    return nil, label .. ' must be a non-negative exactly representable integer'
  end
  return true
end

local function domain_field(value, label)
  if not string_field(value, label, 128) then
    return nil, label .. ' must be a non-empty string of at most 128 bytes'
  end
  if not value:match '^[A-Za-z0-9][A-Za-z0-9_.:-]*$' then
    return nil, label .. ' contains forbidden characters'
  end
  return true
end

local function gui_instance_field(value, label)
  if type(value) ~= 'string' or #value < 2 or #value > 160 or not value:match '^[A-Za-z0-9][A-Za-z0-9_-]+$' then
    return nil, label .. ' must be 2-160 safe ASCII characters'
  end
  return true
end

local function child_ref(value, prefix, label)
  if type(value) ~= 'string' or #value > 180 then
    return nil, label .. ' must be an epoch-qualified child suffix'
  end
  local epoch, handle = value:match('^' .. prefix .. '([0-9a-f%-]+)%.([A-Za-z0-9_-]+)$')
  if not epoch or not epoch:match(UUID) or not handle then
    return nil, label .. ' must be an epoch-qualified child suffix'
  end
  local digits = handle:match '^wz%-(%d+)$' or handle:match '^tx%-(%d+)$'
  if digits and digits:match '^0%d' then
    return nil, label .. ' has a non-canonical provider handle'
  end
  if not digits and not handle:match '^x%-%w[%w_-]*$' then
    return nil, label .. ' has an invalid provider handle'
  end
  return true, epoch
end

local function string_array(value, label, validator, allow_empty)
  if type(value) ~= 'table' then
    return nil, label .. ' must be an array'
  end
  if #value == 0 then
    if allow_empty and canonical.is_array(value) then
      return true
    end
    return nil, label .. ' must be a non-empty array'
  end
  local seen = {}
  for index, item in ipairs(value) do
    local ok, err = validator(item, string.format('%s[%d]', label, index))
    if not ok then
      return nil, err
    end
    if seen[item] then
      return nil, label .. ' contains a duplicate'
    end
    seen[item] = true
  end
  for key in pairs(value) do
    if type(key) ~= 'number' or key % 1 ~= 0 or key < 1 or key > #value then
      return nil, label .. ' must be a dense array'
    end
  end
  return true
end

local function domain_incarnation(value, label)
  local ok, err = exact_keys(value, { name = true, backend_instance_uid = true, server_epoch = true }, label)
  if not ok then
    return nil, err
  end
  ok, err = require_fields(value, { 'name', 'backend_instance_uid', 'server_epoch' }, label)
  if not ok then
    return nil, err
  end
  ok, err = domain_field(value.name, label .. '.name')
  if not ok then
    return nil, err
  end
  for _, field in ipairs { 'backend_instance_uid', 'server_epoch' } do
    ok, err = uuid_field(value[field], label .. '.' .. field)
    if not ok then
      return nil, err
    end
  end
  return true
end

local function domain_incarnation_array(value, label, allow_empty)
  if type(value) ~= 'table' or not canonical.is_array(value) then
    return nil, label .. ' must be an array'
  end
  if #value == 0 and not allow_empty then
    return nil, label .. ' must be non-empty'
  end
  local seen = {}
  for index, item in ipairs(value) do
    local ok, err = domain_incarnation(item, string.format('%s[%d]', label, index))
    if not ok then
      return nil, err
    end
    if seen[item.name] then
      return nil, label .. ' contains a duplicate domain'
    end
    seen[item.name] = true
  end
  for key in pairs(value) do
    if type(key) ~= 'number' or key % 1 ~= 0 or key < 1 or key > #value then
      return nil, label .. ' must be a dense array'
    end
  end
  return true
end

local function require_fields(value, fields, label)
  for _, field in ipairs(fields) do
    if value[field] == nil then
      return nil, string.format('%s.%s is required', label, field)
    end
  end
  return true
end

local function validate_origin(origin, instance)
  if type(origin) ~= 'table' or type(origin.kind) ~= 'string' then
    return nil, 'origin must be a discriminated object'
  end
  if origin.kind == 'in_gui' then
    local ok, err = exact_keys(origin, IN_GUI_KEYS, 'origin')
    if not ok then
      return nil, err
    end
    ok, err = require_fields(origin, {
      'gui_instance',
      'pid',
      'process_start_token',
      'pane_id',
      'domain',
      'host_uid',
      'space_uid',
      'space_no',
      'backend',
      'server_epoch',
      'group_ref',
      'split_ref',
    }, 'origin')
    if not ok then
      return nil, err
    end
    ok, err = gui_instance_field(origin.gui_instance, 'origin.gui_instance')
    if not ok then
      return nil, err
    end
    if origin.gui_instance ~= instance then
      return nil, 'origin.gui_instance does not name this bridge consumer'
    end
    ok, err = uint_field(origin.pid, 'origin.pid')
    if not ok or origin.pid == 0 or origin.pid > 4294967295 then
      return nil, 'origin.pid must be a nonzero process ID'
    end
    ok, err = string_field(origin.process_start_token, 'origin.process_start_token', 256)
    if not ok then
      return nil, err
    end
    ok, err = uint_field(origin.pane_id, 'origin.pane_id')
    if not ok then
      return nil, err
    end
    ok, err = domain_field(origin.domain, 'origin.domain')
    if not ok then
      return nil, err
    end
    for _, name in ipairs { 'host_uid', 'space_uid', 'server_epoch' } do
      ok, err = uuid_field(origin[name], 'origin.' .. name)
      if not ok then
        return nil, err
      end
    end
    ok, err = uint_field(origin.space_no, 'origin.space_no')
    if not ok or origin.space_no == 0 then
      return nil, 'origin.space_no must be a nonzero exactly representable integer'
    end
    if origin.backend ~= 'wez' and origin.backend ~= 'tmux' then
      return nil, 'origin.backend must be wez or tmux'
    end
    local group_ok, group_epoch = child_ref(origin.group_ref, 'g', 'origin.group_ref')
    if not group_ok then
      return nil, group_epoch
    end
    local split_ok, split_epoch = child_ref(origin.split_ref, 'p', 'origin.split_ref')
    if not split_ok then
      return nil, split_epoch
    end
    if group_epoch ~= origin.server_epoch or split_epoch ~= origin.server_epoch then
      return nil, 'origin child epoch differs from origin.server_epoch'
    end
    local provider = origin.backend == 'wez' and '%.wz%-' or '%.tx%-'
    if
      (not origin.group_ref:match(provider) and not origin.group_ref:match '%.x%-')
      or (not origin.split_ref:match(provider) and not origin.split_ref:match '%.x%-')
    then
      return nil, 'origin child provider differs from origin.backend'
    end
    if origin.backend == 'tmux' then
      ok, err = uuid_field(origin.tmux_client_uid, 'origin.tmux_client_uid')
      if not ok then
        return nil, err
      end
    elseif origin.tmux_client_uid ~= nil then
      return nil, 'Wez in_gui origin forbids tmux_client_uid'
    end
    return true
  end

  if origin.kind == 'cold_launcher' then
    local ok, err = exact_keys(origin, COLD_KEYS, 'origin')
    if not ok then
      return nil, err
    end
    ok, err = require_fields(origin, {
      'gui_instance',
      'uid',
      'pid',
      'start_token',
      'launcher_request_uid',
      'domain',
      'host_uid',
      'backend_instance_uid',
      'server_epoch',
    }, 'origin')
    if not ok then
      return nil, err
    end
    ok, err = gui_instance_field(origin.gui_instance, 'origin.gui_instance')
    if not ok then
      return nil, err
    end
    if origin.gui_instance ~= instance then
      return nil, 'origin.gui_instance does not name this bridge consumer'
    end
    for _, name in ipairs { 'uid', 'pid' } do
      ok, err = uint_field(origin[name], 'origin.' .. name)
      if not ok then
        return nil, err
      end
    end
    if origin.pid == 0 then
      return nil, 'origin.pid must be nonzero'
    end
    ok, err = string_field(origin.start_token, 'origin.start_token', 256)
    if not ok then
      return nil, err
    end
    ok, err = uuid_field(origin.launcher_request_uid, 'origin.launcher_request_uid')
    if not ok then
      return nil, err
    end
    ok, err = domain_field(origin.domain, 'origin.domain')
    if not ok then
      return nil, err
    end
    for _, name in ipairs { 'host_uid', 'backend_instance_uid', 'server_epoch' } do
      ok, err = uuid_field(origin[name], 'origin.' .. name)
      if not ok then
        return nil, err
      end
    end
    if origin.space_uid ~= nil then
      ok, err = uuid_field(origin.space_uid, 'origin.space_uid')
      if not ok then
        return nil, err
      end
    end
    return true
  end
  if origin.kind == 'resident_gui' then
    local ok, err = exact_keys(origin, RESIDENT_KEYS, 'origin')
    if not ok then
      return nil, err
    end
    ok, err = require_fields(origin, { 'gui_instance', 'pid', 'process_start_token' }, 'origin')
    if not ok then
      return nil, err
    end
    ok, err = gui_instance_field(origin.gui_instance, 'origin.gui_instance')
    if not ok or origin.gui_instance ~= instance then
      return nil, 'resident origin does not name this bridge consumer'
    end
    ok, err = uint_field(origin.pid, 'origin.pid')
    if not ok or origin.pid == 0 or origin.pid > 4294967295 then
      return nil, 'resident origin.pid must be a nonzero process ID'
    end
    return string_field(origin.process_start_token, 'origin.process_start_token', 256)
  end
  return nil, 'origin.kind must be in_gui, resident_gui, or cold_launcher'
end

local function validate_space_target(target, label)
  local ok, err
  for _, name in ipairs { 'host_uid', 'space_uid', 'backend_instance_uid', 'server_epoch' } do
    ok, err = uuid_field(target[name], label .. '.' .. name)
    if not ok then
      return nil, err
    end
  end
  ok, err = domain_field(target.domain, label .. '.domain')
  if not ok then
    return nil, err
  end
  ok, err = string_field(target.workspace, label .. '.workspace', 256)
  if not ok or target.workspace ~= 'dmux:' .. target.host_uid .. ':' .. target.space_uid then
    return nil, label .. '.workspace must exactly bind target HostUid and SpaceUid'
  end
  if target.group_ref then
    local group_epoch
    ok, group_epoch = child_ref(target.group_ref, 'g', label .. '.group_ref')
    if not ok then
      return nil, group_epoch
    end
    if group_epoch ~= target.server_epoch then
      return nil, label .. '.group_ref epoch differs from target.server_epoch'
    end
  end
  if target.split_ref then
    local split_epoch
    ok, split_epoch = child_ref(target.split_ref, 'p', label .. '.split_ref')
    if not ok then
      return nil, split_epoch
    end
    if split_epoch ~= target.server_epoch then
      return nil, label .. '.split_ref epoch differs from target.server_epoch'
    end
    if not target.group_ref then
      return nil, label .. '.group_ref is required with split_ref'
    end
  end
  if target.alternate_domains then
    ok, err = string_array(target.alternate_domains, label .. '.alternate_domains', domain_field)
    if not ok then
      return nil, err
    end
    for _, domain in ipairs(target.alternate_domains) do
      if domain == target.domain then
        return nil, label .. '.alternate_domains contains the selected domain'
      end
    end
  end
  return true
end

local function validate_target(action, target)
  local allowed = TARGET_KEYS_BY_ACTION[action] or TARGET_KEYS
  local ok, err = exact_keys(target, allowed, 'target')
  if not ok then
    return nil, err
  end
  if action == 'establish_resident' then
    ok, err = require_fields(target, { 'domain', 'host_uid', 'backend_instance_uid', 'server_epoch' }, 'target')
    if not ok then
      return nil, err
    end
    ok, err = domain_field(target.domain, 'target.domain')
    if not ok then
      return nil, err
    end
    for _, name in ipairs { 'host_uid', 'backend_instance_uid', 'server_epoch' } do
      ok, err = uuid_field(target[name], 'target.' .. name)
      if not ok then
        return nil, err
      end
    end
    if target.space_uid ~= nil then
      ok, err = uuid_field(target.space_uid, 'target.space_uid')
      if not ok then
        return nil, err
      end
    end
    return true
  end
  if action == 'ping' then
    if next(target) ~= nil then
      return nil, 'ping target must be empty'
    end
    return true
  end
  if action == 'toast' then
    ok, err = require_fields(target, { 'message' }, 'target')
    if not ok then
      return nil, err
    end
    return string_field(target.message, 'target.message', 4096)
  end
  if action == 'attach_domain' or action == 'detach_domain' then
    ok, err = require_fields(target, { 'domain', 'backend_instance_uid', 'server_epoch' }, 'target')
    if not ok then
      return nil, err
    end
    ok, err = domain_field(target.domain, 'target.domain')
    if not ok then
      return nil, err
    end
    for _, name in ipairs { 'backend_instance_uid', 'server_epoch' } do
      ok, err = uuid_field(target[name], 'target.' .. name)
      if not ok then
        return nil, err
      end
    end
    if action == 'attach_domain' and target.alternate_domains then
      ok, err = string_array(target.alternate_domains, 'target.alternate_domains', domain_field)
      if not ok then
        return nil, err
      end
      for _, domain in ipairs(target.alternate_domains) do
        if domain == target.domain then
          return nil, 'target.alternate_domains contains the selected domain'
        end
      end
    end
    return true
  end
  if action == 'activate' or action == 'present' then
    ok, err = require_fields(
      target,
      { 'domain', 'workspace', 'host_uid', 'space_uid', 'backend_instance_uid', 'server_epoch' },
      'target'
    )
    if not ok then
      return nil, err
    end
    ok, err = validate_space_target(target, 'target')
    if not ok then
      return nil, err
    end
    if action == 'activate' and target.alternate_domains then
      return nil, 'activate cannot attach or detach domains'
    end
    return true
  end
  if action == 'focus_pane' then
    ok, err = require_fields(target, {
      'backend',
      'backend_instance_uid',
      'domain',
      'group_ref',
      'host_uid',
      'pane_id',
      'server_epoch',
      'space_no',
      'space_uid',
      'split_ref',
      'tmux_client_uid',
    }, 'target')
    if not ok then
      return nil, err
    end
    if target.backend ~= 'tmux' then
      return nil, 'target.backend must be tmux for focus_pane'
    end
    ok, err = domain_field(target.domain, 'target.domain')
    if not ok then
      return nil, err
    end
    for _, name in ipairs { 'backend_instance_uid', 'host_uid', 'server_epoch', 'space_uid', 'tmux_client_uid' } do
      ok, err = uuid_field(target[name], 'target.' .. name)
      if not ok then
        return nil, err
      end
    end
    ok, err = uint_field(target.pane_id, 'target.pane_id')
    if not ok then
      return nil, err
    end
    ok, err = uint_field(target.space_no, 'target.space_no')
    if not ok or target.space_no == 0 then
      return nil, 'target.space_no must be a nonzero exactly representable integer'
    end
    local group_ok, group_epoch = child_ref(target.group_ref, 'g', 'target.group_ref')
    if not group_ok then
      return nil, group_epoch
    end
    local split_ok, split_epoch = child_ref(target.split_ref, 'p', 'target.split_ref')
    if not split_ok then
      return nil, split_epoch
    end
    if group_epoch ~= target.server_epoch or split_epoch ~= target.server_epoch then
      return nil, 'target child epoch differs from target.server_epoch'
    end
    if not target.group_ref:match '%.tx%-' and not target.group_ref:match '%.x%-' then
      return nil, 'target.group_ref provider differs from target.backend'
    end
    if not target.split_ref:match '%.tx%-' and not target.split_ref:match '%.x%-' then
      return nil, 'target.split_ref provider differs from target.backend'
    end
    return true
  end
  if action == 'safe_quit' then
    ok, err = require_fields(target, { 'phase' }, 'target')
    if not ok then
      return nil, err
    end
    if target.phase == 'detach' then
      local phase_ok, phase_err = exact_keys(target, { phase = true, domains = true }, 'target')
      if not phase_ok then
        return nil, phase_err
      end
      if not target.domains then
        return nil, 'safe_quit detach requires target.domains'
      end
      -- The sole empty-array authorization in bridge v1 is a safe-quit
      -- no-op detach proof.  The strict decoder preserves [] distinctly
      -- from {}, so a signed object cannot be reinterpreted as this case.
      return domain_incarnation_array(target.domains, 'target.domains', true)
    end
    if target.phase == 'rollback' then
      local phase_ok, phase_err = exact_keys(target, { phase = true, proof_uid = true }, 'target')
      if not phase_ok then
        return nil, phase_err
      end
      if not target.proof_uid then
        return nil, 'safe_quit rollback requires proof_uid'
      end
      return uuid_field(target.proof_uid, 'target.proof_uid')
    end
    if target.phase == 'finish' then
      local phase_ok, phase_err =
        exact_keys(target, { phase = true, proof_uid = true, platform_action = true }, 'target')
      if not phase_ok then
        return nil, phase_err
      end
      if not target.proof_uid or not target.platform_action then
        return nil, 'safe_quit finish requires proof_uid and platform_action'
      end
      ok, err = uuid_field(target.proof_uid, 'target.proof_uid')
      if not ok then
        return nil, err
      end
      if target.platform_action ~= 'hide' and target.platform_action ~= 'quit' then
        return nil, 'target.platform_action must be hide or quit'
      end
      return true
    end
    return nil, 'target.phase must be detach, rollback, or finish'
  end
  return nil, 'unsupported action'
end

local PANE_ACTIONS = { detach_domain = true, focus_pane = true, safe_quit = true }

function M.validate_and_authenticate(request, key, now, instance)
  local ok, err = exact_keys(request, TOP_KEYS, 'request')
  if not ok then
    return failure('malformed_request', err)
  end
  ok, err = require_fields(request, {
    'protocol_version',
    'uid',
    'action',
    'target',
    'issued_at',
    'expiry',
    'nonce',
    'replay_key',
    'origin',
    'hmac_sha256',
  }, 'request')
  if not ok then
    return failure('malformed_request', err)
  end
  if request.protocol_version ~= M.VERSION then
    return failure('protocol_mismatch', 'protocol_version must be 1')
  end
  ok, err = uuid_field(request.uid, 'request.uid')
  if not ok then
    return failure('malformed_request', err)
  end
  if type(request.action) ~= 'string' or not M.ACTIONS[request.action] then
    return failure('unknown_action', 'action is not allowed by bridge v1')
  end
  -- request.uid is the sole persisted one-use identity (ADR 003). The signed
  -- replay_key is retry correlation/entropy: changing it for the same UID
  -- changes the canonical digest and is a conflict, while an accidental
  -- repeat under a different UID does not invent a second persistence index.
  for _, name in ipairs { 'nonce', 'replay_key' } do
    local value = request[name]
    if type(value) ~= 'string' or #value < 32 or #value > 128 or not value:match '^[0-9a-f]+$' then
      return failure('malformed_request', 'request.' .. name .. ' must be 32-128 lowercase hex characters')
    end
  end
  for _, name in ipairs { 'issued_at', 'expiry' } do
    ok, err = uint_field(request[name], 'request.' .. name)
    if not ok then
      return failure('malformed_request', err)
    end
  end
  if request.expiry <= request.issued_at or request.expiry - request.issued_at > M.MAX_TTL_SECONDS then
    return failure('malformed_request', 'request TTL must be between 1 and 10 seconds')
  end
  ok, err = validate_origin(request.origin, instance)
  if not ok then
    return failure('invalid_origin', err)
  end
  if request.origin.kind == 'cold_launcher' and PANE_ACTIONS[request.action] then
    return failure('origin_not_allowed', request.action .. ' requires an in_gui origin')
  end
  ok, err = validate_target(request.action, request.target)
  if not ok then
    return failure('malformed_request', err)
  end
  if
    request.origin.kind == 'cold_launcher'
    and (
      request.action == 'attach_domain'
      or request.action == 'activate'
      or request.action == 'present'
      or request.action == 'establish_resident'
    )
  then
    if
      request.target.domain ~= request.origin.domain
      or request.target.host_uid ~= request.origin.host_uid
      or request.target.backend_instance_uid ~= request.origin.backend_instance_uid
      or request.target.server_epoch ~= request.origin.server_epoch
      or request.target.space_uid ~= request.origin.space_uid
    then
      return failure('invalid_origin', 'cold launcher domain/backend instance differs from the exact target')
    end
  end
  if request.origin.kind == 'resident_gui' and request.action == 'detach_domain' then
    return failure('origin_not_allowed', 'resident_gui cannot issue standalone detach_domain')
  end
  if
    type(request.hmac_sha256) ~= 'string'
    or not request.hmac_sha256:match '^[0-9a-f][0-9a-f]+$'
    or #request.hmac_sha256 ~= 64
  then
    return failure('malformed_request', 'hmac_sha256 must be 64 lowercase hex characters')
  end
  if type(key) ~= 'string' or #key ~= 32 then
    return failure('bridge_key_invalid', 'bridge key must be exactly 32 bytes')
  end
  local document, canonical_err = canonical.signing_document(request)
  if not document then
    return failure('malformed_request', canonical_err)
  end
  local expected = crypto.hmac_sha256(key, document)
  local digest = crypto.sha256(document)
  if not crypto.constant_time_equal(expected, request.hmac_sha256) then
    return failure('unauthorized', 'request HMAC does not match', digest)
  end
  -- Authenticate before reporting freshness and retain the canonical digest
  -- so an otherwise valid expired request still receives a client-verifiable
  -- typed acknowledgement rather than looking like bridge corruption.
  now = now or os.time()
  if now >= request.expiry then
    return failure('expired', 'request reached its signed expiry', digest)
  end
  if request.issued_at > now + 2 then
    return failure('not_yet_valid', 'request issued_at is too far in the future', digest)
  end
  return request, nil, digest
end

function M.sign(request, key)
  local document, err = canonical.signing_document(request)
  if not document then
    return nil, err
  end
  request.hmac_sha256 = crypto.hmac_sha256(key, document)
  return request, document
end

return M
