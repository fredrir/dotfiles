-- Parse and correlate dmux's v1 pane markers. Markers are locator hints;
-- Rust revalidates every GUI-originated operation against the owner registry
-- and live provider scan before issuing a signed bridge action.
local M = {}

local FIELD_MAP = {
  backend = 'dmux_backend',
  context_version = 'dmux_context_version',
  domain = 'dmux_domain',
  group_ref = 'dmux_group_ref',
  host_uid = 'dmux_host_uid',
  server_epoch = 'dmux_server_epoch',
  space_no = 'dmux_space_no',
  space_uid = 'dmux_space_uid',
  split_ref = 'dmux_split_ref',
}

local UUID = '^[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]$'

local function invalid(code, detail)
  return nil, { code = code, message = detail }
end

local function is_uuid(value)
  return type(value) == 'string' and value:match(UUID) ~= nil
end

local function parse_child(value, kind)
  if type(value) ~= 'string' then
    return nil
  end
  local epoch, handle = value:match('^' .. kind .. '([0-9a-f%-]+)%.([A-Za-z0-9_-]+)$')
  if not epoch or not is_uuid(epoch) then
    return nil
  end
  local provider, numeric = handle:match '^(wz)%-(%d+)$'
  if not provider then
    provider, numeric = handle:match '^(tx)%-(%d+)$'
  end
  if provider then
    if numeric:match '^0%d' then
      return nil
    end
  elseif not handle:match '^x%-%w[%w_-]*$' then
    return nil
  end
  return { epoch = epoch, handle = handle, provider = provider }
end

function M.parse_vars(vars, gui_domain, pane_id)
  if type(vars) ~= 'table' then
    return invalid('missing_marker', 'pane has no dmux user variables')
  end
  local raw = {}
  for name, key in pairs(FIELD_MAP) do
    local value = vars[key]
    if type(value) ~= 'string' then
      return invalid('missing_marker', 'pane marker is missing ' .. key)
    end
    raw[name] = value
  end
  if raw.context_version ~= '1' then
    return invalid('marker_version', 'unsupported DMUX_CONTEXT_VERSION')
  end
  if not is_uuid(raw.host_uid) or not is_uuid(raw.space_uid) or not is_uuid(raw.server_epoch) then
    return invalid('malformed_marker', 'marker identity or epoch is not a canonical UUID')
  end
  if not raw.space_no:match '^[1-9][0-9]*$' then
    return invalid('malformed_marker', 'DMUX_SPACE_NO is not canonical nonzero decimal')
  end
  local space_no = tonumber(raw.space_no)
  if not space_no or space_no % 1 ~= 0 or space_no > 9007199254740991 then
    return invalid('malformed_marker', 'DMUX_SPACE_NO exceeds the exactly representable JSON range')
  end
  if raw.backend ~= 'wez' and raw.backend ~= 'tmux' then
    return invalid('malformed_marker', 'DMUX_BACKEND must be wez or tmux')
  end
  if type(gui_domain) ~= 'string' or #gui_domain == 0 then
    return invalid('malformed_marker', 'GUI pane has no domain')
  end
  -- DMUX_DOMAIN was empty in the P8 bootstrap payload. When populated it is
  -- an additional equality check, never a source from which to guess.
  if raw.domain ~= '' and raw.domain ~= gui_domain then
    return invalid('marker_domain_mismatch', 'marker domain differs from the GUI pane domain')
  end
  local group = parse_child(raw.group_ref, 'g')
  local split = parse_child(raw.split_ref, 'p')
  if not group or not split then
    return invalid('malformed_marker', 'Group/Split marker is not an epoch-qualified child suffix')
  end
  if group.epoch ~= raw.server_epoch or split.epoch ~= raw.server_epoch then
    return invalid('marker_epoch_mismatch', 'child ref epoch differs from DMUX_SERVER_EPOCH')
  end
  local expected_provider = raw.backend == 'wez' and 'wz' or 'tx'
  if
    (group.provider and group.provider ~= expected_provider) or (split.provider and split.provider ~= expected_provider)
  then
    return invalid('marker_backend_mismatch', 'child ref provider differs from DMUX_BACKEND')
  end
  if type(pane_id) ~= 'number' or pane_id % 1 ~= 0 or pane_id < 0 or pane_id > 9007199254740991 then
    return invalid('malformed_marker', 'GUI pane id is invalid')
  end

  return {
    context_version = 1,
    host_uid = raw.host_uid,
    space_uid = raw.space_uid,
    space_no = space_no,
    backend = raw.backend,
    -- Owner MarkerContext preserves the optional provider domain; GUI
    -- orchestration separately carries the actual imported domain.
    domain = raw.domain ~= '' and raw.domain or nil,
    gui_domain = gui_domain,
    gui_pane_id = pane_id,
    server_epoch = raw.server_epoch,
    group_ref = raw.group_ref,
    split_ref = raw.split_ref,
  }
end

function M.from_pane(pane)
  if not pane then
    return invalid('missing_pane', 'no active pane')
  end
  local ok, vars = pcall(function()
    return pane:get_user_vars()
  end)
  if not ok then
    return invalid('marker_read_failed', tostring(vars))
  end
  local domain_ok, domain = pcall(function()
    return pane:get_domain_name()
  end)
  local id_ok, pane_id = pcall(function()
    return pane:pane_id()
  end)
  if not domain_ok or not id_ok then
    return invalid('marker_read_failed', 'cannot read GUI pane identity')
  end
  return M.parse_vars(vars, domain, pane_id)
end

function M.marker_context(context)
  local out = {
    host_uid = context.host_uid,
    space_uid = context.space_uid,
    space_no = context.space_no,
    backend = context.backend,
    server_epoch = context.server_epoch,
    group_ref = context.group_ref,
    split_ref = context.split_ref,
  }
  if context.domain then
    out.domain = context.domain
  end
  return out
end

function M.space_uri(context)
  return string.format('dmux://%s/spaces/%s', context.host_uid, context.space_uid)
end

function M.group_uri(context)
  local parsed = assert(parse_child(context.group_ref, 'g'))
  return string.format('%s/groups/%s/%s', M.space_uri(context), parsed.epoch, parsed.handle)
end

function M.split_uri(context)
  local parsed = assert(parse_child(context.split_ref, 'p'))
  return string.format('%s/splits/%s/%s', M.space_uri(context), parsed.epoch, parsed.handle)
end

function M.fingerprint(context)
  return table.concat({
    context.host_uid,
    context.space_uid,
    context.backend,
    context.server_epoch,
    context.gui_domain,
    context.group_ref,
  }, '\0')
end

function M.matches_target(context, target)
  return context.host_uid == target.host_uid
    and context.space_uid == target.space_uid
    and context.server_epoch == target.server_epoch
    and context.gui_domain == target.domain
end

-- Status MIXED means more than one valid logical context is present in the
-- physical tab. Invalid/unstamped peers are reported separately so they can
-- never make an invalid active marker look authoritative.
function M.tab_summary(tab)
  local fingerprints = {}
  local valid = 0
  local invalid_count = 0
  local panes_ok, panes = pcall(function()
    return tab:panes()
  end)
  if not panes_ok then
    return { mixed = false, valid = 0, invalid = 1 }
  end
  for _, pane in ipairs(panes) do
    local context = M.from_pane(pane)
    if context then
      valid = valid + 1
      fingerprints[M.fingerprint(context)] = true
    else
      invalid_count = invalid_count + 1
    end
  end
  local distinct = 0
  for _ in pairs(fingerprints) do
    distinct = distinct + 1
  end
  return { mixed = distinct > 1, valid = valid, invalid = invalid_count }
end

return M
