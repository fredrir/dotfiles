local correlation = require 'wez.dmux_bridge.correlation'
local context = require 'wez.dmux_bridge.context'
local inventory = require 'wez.dmux_bridge.inventory'
local protocol = require 'wez.dmux_bridge.protocol'
local wezterm = require 'wezterm'

local mux = wezterm.mux
local M = {}
local SAFE_QUIT_PROOF_SECONDS = 300
local SAFE_QUIT_ROLLBACK_SECONDS = 4
local SAFE_QUIT_RETRY_MAX_SECONDS = 30

local function fail(done, code, message)
  done(nil, { code = code, message = message })
end

local function same_in_gui_origin(left, right)
  if type(left) ~= 'table' or type(right) ~= 'table' then
    return false
  end
  for _, field in ipairs {
    'kind',
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
    'tmux_client_uid',
  } do
    if left[field] ~= right[field] then
      return false
    end
  end
  return true
end

local function same_execution_origin(left, right)
  if type(left) ~= 'table' or type(right) ~= 'table' or left.kind ~= right.kind then
    return false
  end
  if left.kind == 'resident_gui' then
    return left.gui_instance == right.gui_instance
      and left.pid == right.pid
      and left.process_start_token == right.process_start_token
  end
  return same_in_gui_origin(left, right)
end

local function marker_matches_origin(marker, origin)
  return marker
    and marker.gui_pane_id == origin.pane_id
    and marker.gui_domain == origin.domain
    and marker.host_uid == origin.host_uid
    and marker.space_uid == origin.space_uid
    and marker.space_no == origin.space_no
    and marker.backend == origin.backend
    and marker.server_epoch == origin.server_epoch
    and marker.group_ref == origin.group_ref
    and marker.split_ref == origin.split_ref
    and marker.tmux_client_uid == origin.tmux_client_uid
end

-- HMAC authenticates the request producer, not mutable GUI pane state. Repeat
-- exact pane/process validation in the same callback as every side effect.
local function revalidate_execution_origin(request, state)
  local origin = request.origin
  if type(origin) ~= 'table' then
    return nil, { code = 'invalid_origin', message = 'signed origin is unavailable at execution' }
  end
  if origin.kind == 'cold_launcher' then
    if origin.gui_instance ~= state.id or state.bridge == nil then
      return nil, { code = 'invalid_origin', message = 'cold launcher targets another GUI bridge instance' }
    end
    state.cold_launchers = state.cold_launchers or {}
    local consumed_by = state.cold_launchers[origin.launcher_request_uid]
    if consumed_by ~= nil then
      if consumed_by == request.uid then
        return true
      end
      return nil, { code = 'launcher_witness_replayed', message = 'cold launcher witness was already consumed' }
    end
    local ok, result = pcall(function()
      return state.bridge:consume_launcher_witness(origin)
    end)
    if not ok then
      return nil,
        {
          code = 'launcher_witness_invalid',
          message = 'cold launcher witness is absent, stale, mismatched, or already consumed: ' .. tostring(result),
        }
    end
    local brokered_ok, brokered = pcall(function()
      return state.bridge:resident_brokered()
    end)
    if not brokered_ok or brokered ~= true then
      return nil, { code = 'resident_unbrokered', message = 'launcher witness did not establish resident provenance' }
    end
    state.cold_launchers[origin.launcher_request_uid] = request.uid
    return true
  end
  if origin.kind == 'resident_gui' then
    if
      origin.gui_instance ~= state.id
      or origin.pid ~= state.pid
      or origin.process_start_token ~= state.process_start_token
    then
      return nil, { code = 'invalid_origin', message = 'resident GUI process incarnation changed before execution' }
    end
    local ok, brokered = pcall(function()
      return state.bridge:resident_brokered()
    end)
    if not ok or brokered ~= true then
      return nil, { code = 'resident_unbrokered', message = 'resident GUI lacks a broker-established launch witness' }
    end
    if request.action == 'safe_quit' and request.target.phase ~= 'detach' then
      local prepared = state.safe_quit[request.target.proof_uid]
      if prepared and same_execution_origin(prepared.origin, origin) then
        return true
      end
      return nil, { code = 'quit_proof_missing', message = 'safe_quit continuation lost its resident proof' }
    end
    return true
  end
  if origin.kind ~= 'in_gui' then
    return nil, { code = 'invalid_origin', message = 'unsupported execution origin kind' }
  end
  if
    origin.gui_instance ~= state.id
    or origin.pid ~= state.pid
    or origin.process_start_token ~= state.process_start_token
  then
    return nil, { code = 'invalid_origin', message = 'GUI process incarnation changed before execution' }
  end
  if request.action == 'safe_quit' and request.target.phase ~= 'detach' then
    local prepared = state.safe_quit[request.target.proof_uid]
    if prepared and same_execution_origin(prepared.origin, origin) then
      return true
    end
    return nil, { code = 'quit_proof_missing', message = 'safe_quit continuation lost its exact origin proof' }
  end

  local pane_matches = 0
  local exact_matches = 0
  for _, window in ipairs(mux.all_windows()) do
    for _, tab in ipairs(window:tabs()) do
      for _, pane in ipairs(tab:panes()) do
        local pane_ok, pane_id = pcall(function()
          return pane:pane_id()
        end)
        if pane_ok and pane_id == origin.pane_id then
          pane_matches = pane_matches + 1
          local marker = context.from_pane(pane)
          if marker_matches_origin(marker, origin) then
            exact_matches = exact_matches + 1
          end
        end
      end
    end
  end
  if pane_matches ~= 1 or exact_matches ~= 1 then
    return nil,
      {
        code = 'invalid_origin',
        message = 'origin pane was removed, reused, duplicated, or changed before execution',
      }
  end
  return true
end

local function domain_named(name)
  local ok, domain = pcall(mux.get_domain, name)
  if not ok or not domain then
    return nil
  end
  return domain
end

local function domain_state(domain)
  local ok, state = pcall(function()
    return domain:state()
  end)
  return ok and state or nil
end

local function domain_has_any_panes(domain)
  local ok, has_panes = pcall(function()
    return domain:has_any_panes()
  end)
  if not ok or type(has_panes) ~= 'boolean' then
    return nil
  end
  return has_panes
end

local function wait_until(deadline, predicate, done, timeout_code, timeout_message)
  -- Freshness is checked before success so a state change observed only after
  -- the signed deadline can never turn an expired request into an ack.
  if os.time() >= deadline then
    fail(done, timeout_code, timeout_message)
    return
  end
  local ok, result, err = pcall(predicate)
  if not ok then
    fail(done, 'bridge_internal', tostring(result))
    return
  end
  if result then
    done(result)
    return
  end
  if err then
    done(nil, err)
    return
  end
  wezterm.time.call_after(protocol.POLL_SECONDS, function()
    local callback_ok, callback_err = pcall(wait_until, deadline, predicate, done, timeout_code, timeout_message)
    if not callback_ok then
      fail(done, 'bridge_internal', tostring(callback_err))
    end
  end)
end

local function workspace_in_domain(workspace, domain_name)
  local found = 0
  for _, window in ipairs(mux.all_windows()) do
    if window:get_workspace() == workspace then
      found = found + 1
      for _, tab in ipairs(window:tabs()) do
        for _, pane in ipairs(tab:panes()) do
          if pane:get_domain_name() ~= domain_name then
            return nil, { code = 'workspace_domain_mismatch', message = 'workspace is imported from another domain' }
          end
        end
      end
    end
  end
  if found > 1 then
    return nil, { code = 'ambiguous_workspace', message = 'workspace appears in multiple GUI-local windows' }
  end
  return found == 1
end

local function sentinel_present(target)
  return workspace_in_domain('dmux:system:' .. target.server_epoch, target.domain)
end

local function system_epoch_for_domain(domain_name)
  local epoch
  local windows = 0
  local panes = 0
  for _, window in ipairs(mux.all_windows()) do
    local workspace = window:get_workspace()
    local candidate = type(workspace) == 'string' and workspace:match '^dmux:system:([0-9a-f%-]+)$' or nil
    if candidate then
      local in_domain = false
      local only_domain = true
      local window_panes = 0
      for _, tab in ipairs(window:tabs()) do
        for _, pane in ipairs(tab:panes()) do
          window_panes = window_panes + 1
          if pane:get_domain_name() == domain_name then
            in_domain = true
          else
            only_domain = false
          end
        end
      end
      if in_domain then
        if not only_domain then
          return nil, { code = 'workspace_domain_mismatch', message = 'system workspace crosses domains' }
        end
        panes = panes + window_panes
        windows = windows + 1
        if epoch and epoch ~= candidate then
          return nil, { code = 'domain_inventory_invalid', message = 'domain has multiple system epochs' }
        end
        epoch = candidate
      end
    end
  end
  if windows == 0 then
    return nil
  end
  if windows ~= 1 or panes ~= 1 then
    return nil, { code = 'domain_inventory_invalid', message = 'domain must have one exact system sentinel pane' }
  end
  return epoch
end

local function configured_incarnation(state, target)
  local name = target.name or target.domain
  local expected = state.persistent_domain_instances and state.persistent_domain_instances[name]
  if type(expected) ~= 'string' or expected ~= target.backend_instance_uid then
    return nil,
      {
        code = 'wrong_backend_instance',
        message = 'domain is no longer bound to the signed backend instance: ' .. tostring(name),
      }
  end
  local domain = domain_named(name)
  if not domain then
    return nil, { code = 'no_such_domain', message = 'configured domain is absent: ' .. tostring(name) }
  end
  local state_name = domain_state(domain)
  local has_panes = domain_has_any_panes(domain)
  if
    state_name == nil
    or has_panes == nil
    or (state_name ~= 'Attached' and state_name ~= 'Detached')
    or (state_name == 'Detached' and has_panes)
  then
    return nil, { code = 'domain_inventory_unstable', message = 'domain state cannot be proved safe: ' .. name }
  end
  if state_name == 'Attached' then
    local epoch, epoch_err = system_epoch_for_domain(name)
    if epoch_err then
      return nil, epoch_err
    end
    if epoch ~= target.server_epoch then
      return nil,
        { code = 'backend_epoch_changed', message = 'domain system sentinel differs from the signed server epoch' }
    end
  end
  return domain, state_name
end

local function incarnation_names(records)
  local names = {}
  for _, record in ipairs(records or {}) do
    table.insert(names, record.name or record.domain)
  end
  return names
end

local function detach_domains(records, state, deadline, authorize, done)
  local detached = {}
  local index = 1
  local function step()
    if index > #(records or {}) then
      done({ detached_domains = incarnation_names(records) }, nil, detached)
      return
    end
    local record = records[index]
    local domain, state_or_err = configured_incarnation(state, record)
    if not domain then
      done(nil, state_or_err, detached)
      return
    end
    if state_or_err == 'Detached' then
      index = index + 1
      step()
      return
    end
    local authorized, origin_err = authorize()
    if not authorized then
      done(nil, origin_err, detached)
      return
    end
    -- Incarnation and sentinel were re-read immediately before this side
    -- effect; no stale signed name can detach a rebound domain.
    local ok, err = pcall(function()
      domain:detach()
    end)
    if not ok then
      done(nil, { code = 'detach_failed', message = tostring(err) }, detached)
      return
    end
    wait_until(deadline, function()
      return domain_state(domain) == 'Detached' and domain_has_any_panes(domain) == false
    end, function(_, wait_err)
      if domain_state(domain) == 'Detached' and domain_has_any_panes(domain) == false then
        table.insert(detached, record)
      end
      if wait_err then
        done(nil, wait_err, detached)
        return
      end
      index = index + 1
      step()
    end, 'detach_timeout', 'domain did not become detached before the deadline')
  end
  step()
end

local function reattach_domains(records, state, deadline, done)
  local reattached = {}
  local index = 1
  local function step()
    if index > #(records or {}) then
      done { reattached_domains = incarnation_names(reattached) }
      return
    end
    local record = records[index]
    local domain, state_or_err = configured_incarnation(state, record)
    if not domain then
      done(nil, state_or_err)
      return
    end
    local attached_here = false
    if state_or_err == 'Detached' then
      local ok, err = pcall(function()
        domain:attach()
      end)
      if not ok then
        done(nil, { code = 'rollback_attach_failed', message = tostring(err) })
        return
      end
      attached_here = true
    end
    wait_until(deadline, function()
      if domain_state(domain) ~= 'Attached' then
        return false
      end
      local epoch, epoch_err = system_epoch_for_domain(record.name or record.domain)
      if epoch_err then
        return nil, epoch_err
      end
      return epoch == record.server_epoch
    end, function(_, wait_err)
      if wait_err then
        if not attached_here then
          done(nil, wait_err)
          return
        end
        -- If the detached name rebound while rollback was pending, contain
        -- the wrong incarnation immediately. Never leave a failed rollback
        -- attached merely because its sentinel proved a new epoch.
        local detached_ok, detached_err = pcall(function()
          domain:detach()
        end)
        if not detached_ok then
          done(nil, {
            code = 'rollback_containment_failed',
            message = wait_err.message .. '; cannot re-detach mismatched incarnation: ' .. tostring(detached_err),
          })
          return
        end
        wait_until(os.time() + 4, function()
          return domain_state(domain) == 'Detached' and domain_has_any_panes(domain) == false
        end, function(_, containment_err)
          if containment_err then
            done(nil, {
              code = 'rollback_containment_failed',
              message = wait_err.message .. '; mismatched incarnation did not return to detached state',
            })
          else
            done(nil, wait_err)
          end
        end, 'rollback_containment_failed', 'mismatched incarnation did not return to detached state')
        return
      end
      table.insert(reattached, record)
      index = index + 1
      step()
    end, 'rollback_timeout', 'exact detached domain did not reattach before the deadline')
  end
  step()
end

-- A detached-domain proof owns its recovery even when the controller dies
-- after the detach acknowledgement. Each attempt is bounded; transient
-- attach/provider failures back off while this exact GUI lease and proof
-- remain live. Incarnation mismatches are contained by reattach_domains and
-- therefore never leave a rebound domain attached.
local function schedule_safe_quit_rollback(state, proof_uid, proof, delay, reason)
  if state.safe_quit[proof_uid] ~= proof or proof.rollback_scheduled then
    return
  end
  proof.rollback_scheduled = true
  wezterm.time.call_after(delay, function()
    proof.rollback_scheduled = false
    if state.safe_quit[proof_uid] ~= proof or proof.rolling_back then
      return
    end
    local lease_ok, identity = pcall(function()
      return state.bridge:identity()
    end)
    if
      not lease_ok
      or type(identity) ~= 'table'
      or identity.gui_instance ~= state.id
      or identity.pid ~= state.pid
      or identity.process_start_token ~= state.process_start_token
    then
      wezterm.log_error 'dmux bridge: safe-quit rollback paused because the exact GUI lease is unavailable'
      schedule_safe_quit_rollback(
        state,
        proof_uid,
        proof,
        math.min(math.max(delay * 2, 1), SAFE_QUIT_RETRY_MAX_SECONDS),
        reason
      )
      return
    end
    proof.rolling_back = true
    reattach_domains(proof.domains, state, os.time() + SAFE_QUIT_ROLLBACK_SECONDS, function(_, rollback_err)
      proof.rolling_back = false
      if state.safe_quit[proof_uid] ~= proof then
        return
      end
      if rollback_err then
        wezterm.log_error(
          'dmux bridge: safe-quit ' .. tostring(reason) .. ' rollback failed closed; retrying: ' .. rollback_err.message
        )
        schedule_safe_quit_rollback(
          state,
          proof_uid,
          proof,
          math.min(math.max(delay * 2, 1), SAFE_QUIT_RETRY_MAX_SECONDS),
          reason
        )
      else
        state.safe_quit[proof_uid] = nil
      end
    end)
  end)
end

local function retain_safe_quit_recovery(state, request, domains, reason)
  if #(domains or {}) == 0 then
    return
  end
  local proof = {
    domains = domains,
    origin = request.origin,
    expires_at = os.time(),
  }
  state.safe_quit[request.uid] = proof
  schedule_safe_quit_rollback(state, request.uid, proof, 1, reason)
end

local function same_incarnations(left, right)
  if #left ~= #right then
    return false
  end
  local expected = {}
  for _, record in ipairs(left) do
    expected[record.name] = record.backend_instance_uid .. ':' .. record.server_epoch
  end
  for _, record in ipairs(right) do
    if expected[record.name] ~= record.backend_instance_uid .. ':' .. record.server_epoch then
      return false
    end
  end
  return true
end

local function persistent_snapshot(state)
  local configured = inventory.configured_set(state.persistent_domains)
  if not configured then
    return nil, { code = 'bridge_internal', message = 'persistent domain authority is unavailable' }
  end
  local seen, active = {}, {}
  for _, domain in ipairs(mux.all_domains()) do
    local name_ok, name = pcall(function()
      return domain:name()
    end)
    if not name_ok or type(name) ~= 'string' or #name == 0 then
      return nil, { code = 'domain_inventory_invalid', message = 'GUI domain inventory is ambiguous' }
    end
    -- Non-routable rows are WezTerm's connection-UI placeholder domain, which
    -- the mux leaks one of per attach and never frees. They are skipped before
    -- identity is proved, so neither a permanently `Attached` placeholder nor a
    -- second copy sharing its name can refuse the snapshot.
    if inventory.routable(domain, name, configured) then
      if seen[name] then
        return nil, { code = 'domain_inventory_invalid', message = 'GUI domain inventory is ambiguous' }
      end
      seen[name] = true
      local state_name = domain_state(domain)
      local has_panes = domain_has_any_panes(domain)
      if state_name == nil or has_panes == nil then
        return nil, { code = 'domain_inventory_invalid', message = 'GUI domain state cannot be proven' }
      end
      if name ~= 'local' then
        if configured[name] then
          if state_name ~= 'Attached' and state_name ~= 'Detached' then
            return nil,
              {
                code = 'domain_inventory_unstable',
                message = 'configured persistent domain is in a transient or failed state: ' .. name,
              }
          end
          if state_name == 'Detached' and has_panes then
            return nil,
              {
                code = 'domain_inventory_invalid',
                message = 'detached persistent domain still reports panes: ' .. name,
              }
          end
          if state_name == 'Attached' or has_panes then
            local epoch, epoch_err = system_epoch_for_domain(name)
            if epoch_err then
              return nil, epoch_err
            end
            if not epoch then
              return nil,
                { code = 'domain_inventory_invalid', message = 'active domain has no exact system sentinel: ' .. name }
            end
            table.insert(active, {
              name = name,
              backend_instance_uid = state.persistent_domain_instances[name],
              server_epoch = epoch,
            })
          end
        elseif state_name ~= 'Detached' or has_panes then
          return nil,
            {
              code = 'unknown_persistent_domain',
              message = 'active non-local domain is outside the sanitized dmux configuration: ' .. name,
            }
        end
      end
    end
  end
  for name in pairs(configured) do
    if not seen[name] then
      return nil, { code = 'domain_inventory_invalid', message = 'configured persistent domain is absent: ' .. name }
    end
  end
  table.sort(active, function(left, right)
    return left.name < right.name
  end)
  return active
end

local function alternate_routes_clear(target, bridge_state)
  for _, name in ipairs(target.alternate_domains or {}) do
    if
      not bridge_state.persistent_domain_instances
      or bridge_state.persistent_domain_instances[name] ~= target.backend_instance_uid
    then
      return nil,
        { code = 'wrong_backend_instance', message = 'alternate route no longer aliases the signed backend: ' .. name }
    end
    local domain = domain_named(name)
    if not domain then
      return nil, { code = 'no_such_domain', message = 'configured alternate domain is absent: ' .. name }
    end
    local state = domain_state(domain)
    local has_panes = domain_has_any_panes(domain)
    if state == nil or has_panes == nil or (state ~= 'Attached' and state ~= 'Detached') then
      return nil, { code = 'domain_inventory_unstable', message = 'alternate domain state cannot be proven: ' .. name }
    end
    if state == 'Attached' or has_panes then
      return nil,
        {
          code = 'alternate_route_attached',
          message = 'refusing to attach a second route while a compatible alternate is still active: ' .. name,
        }
    end
  end
  return true
end

local function attach_domain(target, bridge_state, deadline, authorize, done)
  if
    not bridge_state.persistent_domain_instances
    or bridge_state.persistent_domain_instances[target.domain] ~= target.backend_instance_uid
  then
    fail(done, 'wrong_backend_instance', 'selected domain no longer binds the signed backend instance')
    return
  end
  local selected = domain_named(target.domain)
  if not selected then
    fail(done, 'no_such_domain', 'configured domain is absent: ' .. target.domain)
    return
  end
  local clear, clear_err = alternate_routes_clear(target, bridge_state)
  if not clear then
    done(nil, clear_err)
    return
  end
  local selected_state = domain_state(selected)
  local selected_has_panes = domain_has_any_panes(selected)
  if
    selected_state == nil
    or selected_has_panes == nil
    or (selected_state ~= 'Attached' and selected_state ~= 'Detached')
    or (selected_state == 'Detached' and selected_has_panes)
  then
    fail(done, 'domain_inventory_unstable', 'selected domain state cannot be proved safe: ' .. target.domain)
    return
  end
  local function attach_selected()
    if selected_state ~= 'Attached' then
      local authorized, origin_err = authorize()
      if not authorized then
        done(nil, origin_err)
        return
      end
      local ok, err = pcall(function()
        -- Selected zero-window primitive: unlike AttachDomain this does not
        -- spawn when a domain is empty.
        selected:attach()
      end)
      if not ok then
        fail(done, 'attach_failed', tostring(err))
        return
      end
    end
    wait_until(deadline, function()
      if domain_state(selected) ~= 'Attached' then
        return false
      end
      local present, sentinel_err = sentinel_present(target)
      if sentinel_err then
        return nil, sentinel_err
      end
      if not present then
        return false
      end
      return { domain = target.domain, domain_state = 'Attached' }
    end, function(result, attach_err)
      if attach_err then
        done(nil, attach_err)
        return
      end
      local still_clear, alternate_err = alternate_routes_clear(target, bridge_state)
      if not still_clear then
        done(nil, alternate_err)
        return
      end
      done(result)
    end, 'attach_timeout', 'domain/sentinel did not appear before the deadline')
  end
  attach_selected()
end

local function activation_result(result)
  return {
    domain = result.domain,
    workspace = result.workspace,
    window_ids = result.window_ids,
    pane_id = result.pane_id,
    group_ref = result.group_ref,
    split_ref = result.split_ref,
  }
end

local function activate(target, done)
  local result, err = correlation.activate(mux, target)
  if not result then
    done(nil, err)
    return
  end
  done(activation_result(result))
end

local function marker_snapshot_incomplete(err)
  return type(err) == 'table'
    and err.code == 'invalid_marker'
    and type(err.message) == 'string'
    and err.message:find('pane marker is missing dmux_', 1, true) ~= nil
end

local function present(target, bridge_state, deadline, authorize, done)
  attach_domain(target, bridge_state, deadline, authorize, function(_, attach_err)
    if attach_err then
      done(nil, attach_err)
      return
    end
    local incomplete_marker
    wait_until(deadline, function()
      local exists, workspace_err = workspace_in_domain(target.workspace, target.domain)
      if workspace_err then
        return nil, workspace_err
      end
      if not exists then
        return false
      end
      local authorized, origin_err = authorize()
      if not authorized then
        return nil, origin_err
      end
      local result, activate_err = correlation.activate(mux, target)
      if result then
        return activation_result(result)
      end
      if marker_snapshot_incomplete(activate_err) then
        incomplete_marker = activate_err
        return false
      end
      return nil, activate_err
    end, function(result, wait_err)
      if wait_err then
        -- A newly imported pane may be observable one event-loop turn before
        -- its server-side SetUserVar snapshot.  Retry only that exact partial
        -- marker shape; a stable incomplete marker still fails closed with
        -- its original typed error rather than being mislabeled not_found.
        done(nil, incomplete_marker or wait_err)
      else
        done(result)
      end
    end, 'not_found', 'opaque workspace did not appear before the deadline')
  end)
end

local function focus_pane(target, done)
  local result, err = correlation.focus_pane(mux, target)
  if not result then
    done(nil, err)
    return
  end
  done(result)
end

local function first_gui_window()
  local ok, windows = pcall(function()
    return wezterm.gui.gui_windows()
  end)
  if not ok or #windows == 0 then
    return nil, nil
  end
  local window = windows[1]
  local pane_ok, pane = pcall(function()
    return window:active_pane()
  end)
  return window, pane_ok and pane or nil
end

local function safe_quit(request, state, deadline, authorize, done)
  local target = request.target
  if target.phase == 'detach' then
    local active, inventory_err = persistent_snapshot(state)
    if not active then
      done(nil, inventory_err)
      return
    end
    if not same_incarnations(active, target.domains) then
      fail(done, 'quit_domain_snapshot_mismatch', 'safe_quit domain set is stale or incomplete')
      return
    end
    detach_domains(target.domains, state, deadline, authorize, function(result, err, detached)
      if err then
        reattach_domains(detached, state, os.time() + 4, function(_, rollback_err)
          if rollback_err then
            retain_safe_quit_recovery(state, request, detached, 'partial detach failure')
            done(nil, {
              code = 'detach_rollback_failed',
              message = err.message .. '; exact-incarnation rollback failed: ' .. rollback_err.message,
            })
          else
            done(nil, err)
          end
        end)
        return
      end
      local still_active, post_err = persistent_snapshot(state)
      if not still_active or #still_active ~= 0 then
        local failure = post_err
          or { code = 'quit_domain_reattached', message = 'a persistent domain attached during safe_quit detach' }
        reattach_domains(target.domains, state, os.time() + 4, function(_, rollback_err)
          if rollback_err then
            retain_safe_quit_recovery(state, request, target.domains, 'post-detach proof failure')
            done(nil, {
              code = 'detach_rollback_failed',
              message = failure.message .. '; exact-incarnation rollback failed: ' .. rollback_err.message,
            })
          else
            done(nil, failure)
          end
        end)
        return
      end
      local proof = {
        domains = target.domains,
        origin = request.origin,
        expires_at = os.time() + SAFE_QUIT_PROOF_SECONDS,
      }
      state.safe_quit[request.uid] = proof
      schedule_safe_quit_rollback(state, request.uid, proof, SAFE_QUIT_PROOF_SECONDS, 'expired proof')
      done(result)
    end)
    return
  end

  local prepared = state.safe_quit[target.proof_uid]
  if not prepared then
    fail(done, 'quit_proof_missing', 'safe_quit finish does not match a completed detach phase')
    return
  end
  if os.time() >= prepared.expires_at then
    prepared.rolling_back = true
    reattach_domains(prepared.domains, state, os.time() + 4, function(result, rollback_err)
      prepared.rolling_back = false
      if rollback_err then
        schedule_safe_quit_rollback(state, target.proof_uid, prepared, 1, 'expired proof')
        done(nil, {
          code = 'quit_proof_expired_rollback_failed',
          message = 'safe_quit proof expired and exact-incarnation rollback failed: ' .. rollback_err.message,
        })
        return
      end
      state.safe_quit[target.proof_uid] = nil
      if target.phase == 'rollback' then
        done(result)
      else
        fail(done, 'quit_proof_expired', 'safe_quit proof expired; domains were restored')
      end
    end)
    return
  end
  if target.phase == 'rollback' then
    reattach_domains(prepared.domains, state, os.time() + 4, function(result, rollback_err)
      if rollback_err then
        schedule_safe_quit_rollback(state, target.proof_uid, prepared, 1, 'explicit rollback')
        done(nil, rollback_err)
        return
      end
      state.safe_quit[target.proof_uid] = nil
      done(result)
    end)
    return
  end
  local expected_action = wezterm.target_triple:find('darwin', 1, true) and 'hide' or 'quit'
  if target.platform_action ~= expected_action then
    fail(
      done,
      'quit_platform_mismatch',
      string.format('safe_quit requires platform_action=%s on this GUI', expected_action)
    )
    return
  end
  local active, inventory_err = persistent_snapshot(state)
  if not active then
    done(nil, inventory_err)
    return
  end
  if #active ~= 0 then
    fail(done, 'quit_domain_reattached', 'a persistent domain is no longer detached and pane-free')
    return
  end

  local window = first_gui_window()
  if target.platform_action == 'hide' and not window then
    done {
      platform_action = 'hide',
      already_hidden = true,
      after_ack = function()
        state.safe_quit[target.proof_uid] = nil
      end,
    }
    return
  end
  local method_ok, completion = pcall(function()
    return state.bridge.complete_safe_lifecycle
  end)
  if not method_ok or type(completion) ~= 'function' then
    fail(done, 'managed_quit_unavailable', 'maintained fork lacks the proved safe lifecycle completion primitive')
    return
  end
  done {
    platform_action = target.platform_action,
    after_ack = function()
      local ok, accepted = pcall(function()
        return state.bridge:complete_safe_lifecycle(request.uid, target.platform_action)
      end)
      if not ok or accepted == false then
        wezterm.log_error('dmux bridge: post-ack safe quit action failed: ' .. tostring(accepted))
        schedule_safe_quit_rollback(state, target.proof_uid, prepared, 1, 'native completion failure')
      else
        -- A returning fork primitive means the application-scoped lifecycle
        -- handoff was accepted. A nonreturning quit naturally retains the
        -- proof only until the process exits.
        state.safe_quit[target.proof_uid] = nil
      end
    end,
  }
end

function M.dispatch(request, state, done)
  local deadline = math.min(request.expiry, os.time() + 4)
  local function authorize()
    return revalidate_execution_origin(request, state)
  end
  local authorized, origin_err = authorize()
  if not authorized then
    done(nil, origin_err)
    return
  end
  if request.action == 'establish_resident' then
    done { resident_established = true }
  elseif request.action == 'ping' then
    done { pong = true }
  elseif request.action == 'toast' then
    local window = first_gui_window()
    if not window then
      fail(done, 'no_gui_window', 'there is no GUI window for a toast')
      return
    end
    local ok, err = pcall(function()
      window:toast_notification('dmux', request.target.message, nil, 4000)
    end)
    if ok then
      done { toasted = true }
    else
      fail(done, 'toast_failed', tostring(err))
    end
  elseif request.action == 'attach_domain' then
    attach_domain(request.target, state, deadline, authorize, done)
  elseif request.action == 'detach_domain' then
    detach_domains({ request.target }, state, deadline, authorize, done)
  elseif request.action == 'focus_pane' then
    focus_pane(request.target, done)
  elseif request.action == 'activate' then
    activate(request.target, done)
  elseif request.action == 'present' then
    present(request.target, state, deadline, authorize, done)
  elseif request.action == 'safe_quit' then
    safe_quit(request, state, deadline, authorize, done)
  else
    fail(done, 'unknown_action', 'action is not implemented')
  end
end

return M
