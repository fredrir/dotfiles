local context = require 'wez.dmux_bridge.context'

local M = {}

local function failure(code, message)
  return nil, { code = code, message = message }
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

return M
