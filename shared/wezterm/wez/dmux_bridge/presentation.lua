local correlation = require 'wez.dmux_bridge.correlation'
local protocol = require 'wez.dmux_bridge.protocol'
local wezterm = require 'wezterm'

local mux = wezterm.mux
local M = {}

local function fail(done, code, message)
  done(nil, { code = code, message = message })
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
  if os.time() >= deadline then
    fail(done, timeout_code, timeout_message)
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

local function detach_domains(names, deadline, done)
  for _, name in ipairs(names or {}) do
    local domain = domain_named(name)
    if not domain then
      fail(done, 'no_such_domain', 'configured domain is absent: ' .. name)
      return
    end
    if domain_state(domain) ~= 'Detached' then
      local ok, err = pcall(function()
        domain:detach()
      end)
      if not ok then
        fail(done, 'detach_failed', tostring(err))
        return
      end
    end
  end
  wait_until(deadline, function()
    for _, name in ipairs(names or {}) do
      local domain = domain_named(name)
      if not domain or domain_state(domain) ~= 'Detached' or domain_has_any_panes(domain) ~= false then
        return false
      end
    end
    return { detached_domains = names }
  end, done, 'detach_timeout', 'domain did not become detached before the deadline')
end

local function same_set(left, right)
  if #left ~= #right then
    return false
  end
  local expected = {}
  for _, name in ipairs(left) do
    expected[name] = true
  end
  for _, name in ipairs(right) do
    if not expected[name] then
      return false
    end
  end
  return true
end

local function persistent_snapshot(state)
  if type(state.persistent_domains) ~= 'table' then
    return nil, { code = 'bridge_internal', message = 'persistent domain authority is unavailable' }
  end
  local configured = {}
  for _, name in ipairs(state.persistent_domains) do
    configured[name] = true
  end
  local seen, active = {}, {}
  for _, domain in ipairs(mux.all_domains()) do
    local name_ok, name = pcall(function()
      return domain:name()
    end)
    if not name_ok or type(name) ~= 'string' or #name == 0 or seen[name] then
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
          table.insert(active, name)
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
  for name in pairs(configured) do
    if not seen[name] then
      return nil, { code = 'domain_inventory_invalid', message = 'configured persistent domain is absent: ' .. name }
    end
  end
  table.sort(active)
  return active
end

local function alternate_routes_clear(target)
  for _, name in ipairs(target.alternate_domains or {}) do
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

local function attach_domain(target, deadline, done)
  local selected = domain_named(target.domain)
  if not selected then
    fail(done, 'no_such_domain', 'configured domain is absent: ' .. target.domain)
    return
  end
  local clear, clear_err = alternate_routes_clear(target)
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
      local still_clear, alternate_err = alternate_routes_clear(target)
      if not still_clear then
        done(nil, alternate_err)
        return
      end
      done(result)
    end, 'attach_timeout', 'domain/sentinel did not appear before the deadline')
  end
  attach_selected()
end

local function activate(target, done)
  local result, err = correlation.activate(mux, target)
  if not result then
    done(nil, err)
    return
  end
  done {
    domain = result.domain,
    workspace = result.workspace,
    window_ids = result.window_ids,
    pane_id = result.pane_id,
    group_ref = result.group_ref,
    split_ref = result.split_ref,
  }
end

local function present(target, deadline, done)
  attach_domain(target, deadline, function(_, attach_err)
    if attach_err then
      done(nil, attach_err)
      return
    end
    wait_until(deadline, function()
      local exists, workspace_err = workspace_in_domain(target.workspace, target.domain)
      if workspace_err then
        return nil, workspace_err
      end
      return exists
    end, function(_, wait_err)
      if wait_err then
        done(nil, wait_err)
      else
        activate(target, done)
      end
    end, 'not_found', 'opaque workspace did not appear before the deadline')
  end)
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

local function safe_quit(request, state, deadline, done)
  local target = request.target
  if target.phase == 'detach' then
    local active, inventory_err = persistent_snapshot(state)
    if not active then
      done(nil, inventory_err)
      return
    end
    if not same_set(active, target.domains) then
      fail(done, 'quit_domain_snapshot_mismatch', 'safe_quit domain set is stale or incomplete')
      return
    end
    local window = first_gui_window()
    detach_domains(target.domains, deadline, function(result, err)
      if err then
        done(nil, err)
        return
      end
      local still_active, post_err = persistent_snapshot(state)
      if not still_active then
        done(nil, post_err)
        return
      end
      if #still_active ~= 0 then
        fail(done, 'quit_domain_reattached', 'a persistent domain attached during safe_quit detach')
        return
      end
      state.safe_quit[request.uid] = {
        domains = state.persistent_domains,
        window = window,
        expires_at = os.time() + 30,
      }
      done(result)
    end)
    return
  end

  local prepared = state.safe_quit[target.proof_uid]
  if not prepared then
    fail(done, 'quit_proof_missing', 'safe_quit finish does not match a completed detach phase')
    return
  end
  if os.time() > prepared.expires_at then
    state.safe_quit[target.proof_uid] = nil
    fail(done, 'quit_proof_expired', 'safe_quit detach proof expired before the finish phase')
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
  window = window or prepared.window
  if not window then
    fail(done, 'no_gui_window', 'final quit needs the OS controller after all GUI windows disappeared')
    return
  end
  local completion = target.platform_action == 'hide' and 'dmux_safe_hide_application' or 'dmux_safe_quit_application'
  if type(window[completion]) ~= 'function' then
    fail(done, 'managed_quit_unavailable', 'maintained fork lacks the proved safe lifecycle completion primitive')
    return
  end
  local config_ok, managed = pcall(function()
    return window:effective_config().dmux_managed_gui
  end)
  if not config_ok or managed ~= true then
    fail(done, 'managed_quit_unavailable', 'safe lifecycle completion is not enabled for this managed GUI window')
    return
  end
  done {
    platform_action = target.platform_action,
    after_ack = function()
      state.safe_quit[target.proof_uid] = nil
      local ok, err = pcall(function()
        if target.platform_action == 'hide' then
          window:dmux_safe_hide_application()
        else
          window:dmux_safe_quit_application()
        end
      end)
      if not ok then
        wezterm.log_error('dmux bridge: post-ack safe quit action failed: ' .. tostring(err))
      end
    end,
  }
end

function M.dispatch(request, state, done)
  local deadline = math.min(request.expiry, os.time() + 4)
  if request.action == 'ping' then
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
    attach_domain(request.target, deadline, done)
  elseif request.action == 'detach_domain' then
    detach_domains({ request.target.domain }, deadline, done)
  elseif request.action == 'activate' then
    activate(request.target, done)
  elseif request.action == 'present' then
    present(request.target, deadline, done)
  elseif request.action == 'safe_quit' then
    safe_quit(request, state, deadline, done)
  else
    fail(done, 'unknown_action', 'action is not implemented')
  end
end

return M
