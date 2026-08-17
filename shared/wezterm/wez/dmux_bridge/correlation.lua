local context = require 'wez.dmux_bridge.context'

local M = {}

local function failure(code, message)
  return nil, { code = code, message = message }
end

local function exact_focus_marker(marker, target)
  return marker
    and marker.gui_pane_id == target.pane_id
    and marker.gui_domain == target.domain
    and marker.tmux_client_uid == target.tmux_client_uid
    and marker.host_uid == target.host_uid
    and marker.space_uid == target.space_uid
    and marker.space_no == target.space_no
    and marker.backend == 'tmux'
    and marker.domain == nil
    and marker.server_epoch == target.server_epoch
    and marker.group_ref == target.group_ref
    and marker.split_ref == target.split_ref
end

local function mux_inventory(mux, target)
  local rows, seen, matches = {}, {}, {}
  for _, window in ipairs(mux.all_windows()) do
    local workspace = window:get_workspace()
    local window_id = window:window_id()
    for _, tab in ipairs(window:tabs()) do
      local tab_id = tab:tab_id()
      for _, pane in ipairs(tab:panes()) do
        local pane_id = pane:pane_id()
        local domain = pane:get_domain_name()
        if seen[pane_id] then
          return failure('ambiguous_pane', 'GUI pane id appears more than once in the mux inventory')
        end
        seen[pane_id] = true
        table.insert(
          rows,
          table.concat({ tostring(window_id), tostring(tab_id), tostring(pane_id), workspace, domain }, '\0')
        )
        local marker = context.from_pane(pane)
        if exact_focus_marker(marker, target) then
          table.insert(matches, { window = window, tab = tab, pane = pane, workspace = workspace })
        end
      end
    end
  end
  table.sort(rows)
  return { rows = rows, matches = matches }
end

local function same_rows(left, right)
  if #left ~= #right then
    return false
  end
  for index, value in ipairs(left) do
    if right[index] ~= value then
      return false
    end
  end
  return true
end

local function workspace_windows(mux, workspace)
  local matches = {}
  for _, window in ipairs(mux.all_windows()) do
    if window:get_workspace() == workspace then
      table.insert(matches, window)
    end
  end
  table.sort(matches, function(left, right)
    return left:window_id() < right:window_id()
  end)
  return matches
end

local function matching_panes(window, target)
  local matches = {}
  for _, tab in ipairs(window:tabs()) do
    for _, info in ipairs(tab:panes_with_info()) do
      local marker = context.from_pane(info.pane)
      if marker and context.matches_target(marker, target) and marker.group_ref == target.group_ref then
        table.insert(matches, {
          tab = tab,
          pane = info.pane,
          is_active = info.is_active,
          marker = marker,
        })
      end
    end
  end
  return matches
end

local function validate_workspace_context(window, target)
  local panes = 0
  local target_panes = 0
  for _, tab in ipairs(window:tabs()) do
    for _, pane in ipairs(tab:panes()) do
      panes = panes + 1
      local marker, marker_err = context.from_pane(pane)
      if not marker then
        return failure(
          'invalid_marker',
          'opaque workspace contains an unstamped or malformed pane: ' .. tostring(marker_err and marker_err.message)
        )
      end
      if marker.gui_domain ~= target.domain then
        return failure(
          'workspace_domain_mismatch',
          'opaque workspace contains a pane imported through another GUI domain'
        )
      end
      if context.matches_target(marker, target) then
        target_panes = target_panes + 1
      end
    end
  end
  if panes == 0 then
    return failure('not_found', 'opaque workspace has no panes')
  end
  if target_panes == 0 then
    return failure('workspace_context_mismatch', 'opaque workspace contains no pane from the requested Space and epoch')
  end
  return true
end

function M.resolve(mux, target)
  local windows = workspace_windows(mux, target.workspace)
  if #windows == 0 then
    return failure('not_found', 'opaque workspace is not imported in this GUI')
  end
  if #windows ~= 1 then
    return failure('ambiguous_workspace', 'opaque workspace appears in multiple GUI-local windows')
  end
  local window = windows[1]
  local result = {
    window = window,
    window_ids = { window:window_id() },
    workspace = target.workspace,
    domain = target.domain,
  }
  local valid, validation_err = validate_workspace_context(window, target)
  if not valid then
    return nil, validation_err
  end
  if not target.group_ref then
    return result
  end

  local matches = matching_panes(window, target)
  if #matches == 0 then
    return failure('group_not_found', 'no pane has the requested epoch-qualified Group ref')
  end
  local tabs = {}
  for _, match in ipairs(matches) do
    tabs[match.tab:tab_id()] = match.tab
  end
  local tab_count = 0
  local matched_tab
  for _, tab in pairs(tabs) do
    tab_count = tab_count + 1
    matched_tab = tab
  end
  if tab_count ~= 1 then
    return failure('ambiguous_group', 'the requested Group ref appears in multiple GUI-local tabs')
  end
  result.tab = matched_tab
  result.group_ref = target.group_ref

  if target.split_ref then
    local split_matches = {}
    for _, match in ipairs(matches) do
      if match.marker.split_ref == target.split_ref then
        table.insert(split_matches, match)
      end
    end
    if #split_matches ~= 1 then
      return failure(
        #split_matches == 0 and 'split_not_found' or 'ambiguous_split',
        'the requested Split ref must match exactly one GUI-local pane in its parent Group'
      )
    end
    result.pane = split_matches[1].pane
    result.pane_id = result.pane:pane_id()
    result.split_ref = target.split_ref
    return result
  end

  -- Group-only focus: keep the active matching pane, otherwise choose the
  -- lexicographically smallest canonical Split ref (not a remapped pane id).
  local selected
  local active_matches = 0
  for _, match in ipairs(matches) do
    if match.is_active then
      active_matches = active_matches + 1
      selected = match
    end
  end
  if active_matches > 1 then
    return failure('ambiguous_split', 'the Group reports more than one active matching Split')
  end
  if not selected then
    table.sort(matches, function(left, right)
      return left.marker.split_ref < right.marker.split_ref
    end)
    selected = matches[1]
  end
  result.pane = selected.pane
  result.pane_id = selected.pane:pane_id()
  result.split_ref = selected.marker.split_ref
  return result
end

function M.activate(mux, target)
  local result, err = M.resolve(mux, target)
  if not result then
    return nil, err
  end
  local ok, activate_err = pcall(mux.set_active_workspace, target.workspace)
  if not ok then
    return failure('activate_failed', tostring(activate_err))
  end
  if result.tab then
    ok, activate_err = pcall(function()
      result.tab:activate()
      result.pane:activate()
    end)
    if not ok then
      return failure('focus_failed', tostring(activate_err))
    end
  end
  return result
end

-- Focus an already-visible outer GUI pane without attaching a domain,
-- creating a workspace, or mutating the owner. Pane id alone is never
-- sufficient: the complete tmux marker and attach-time client UID must still
-- identify exactly one pane both before and after activation.
function M.focus_pane(mux, target)
  local ok, before, before_err = pcall(mux_inventory, mux, target)
  if not ok then
    return failure('inventory_failed', tostring(before))
  end
  if not before then
    return nil, before_err
  end
  if #before.matches ~= 1 then
    return failure(
      #before.matches == 0 and 'pane_not_found' or 'ambiguous_pane',
      'focus_pane target must match exactly one GUI pane by id, marker, and tmux client UID'
    )
  end
  local selected = before.matches[1]
  local focus_err
  ok, focus_err = pcall(function()
    mux.set_active_workspace(selected.workspace)
    selected.tab:activate()
    selected.pane:activate()
  end)
  if not ok then
    return failure('focus_failed', tostring(focus_err))
  end
  local after, after_err = mux_inventory(mux, target)
  if not after then
    return nil, after_err
  end
  if not same_rows(before.rows, after.rows) then
    return failure('pane_inventory_changed', 'GUI pane inventory changed during no-create focus')
  end
  if #after.matches ~= 1 then
    return failure('focus_postcondition_failed', 'focused pane marker/client identity changed during activation')
  end
  return {
    domain = target.domain,
    pane_id = target.pane_id,
    group_ref = target.group_ref,
    split_ref = target.split_ref,
  }
end

return M
