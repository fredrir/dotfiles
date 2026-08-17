local controller = require 'wez.dmux_bridge.controller'
local context = require 'wez.dmux_bridge.context'
local wezterm = require 'wezterm'

local act = wezterm.action
local M = {}

local CLIENT_UUID = '^[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]$'

-- Return a fresh argv table carrying any canonical attach-time UID.  It is
-- deliberately independent of the pane marker: immediately after attaching,
-- the outer Wez pane may still contain an older Wez marker.  Rust treats UID
-- plus marker as untrusted inputs and refuses that mismatch fail-closed.
function M.with_tmux_client_uid(pane, args)
  local out = {}
  for _, value in ipairs(args or {}) do
    table.insert(out, value)
  end
  local ok, vars = pcall(function()
    return pane:get_user_vars()
  end)
  local uid = ok and type(vars) == 'table' and vars.dmux_tmux_client_uid or nil
  if type(uid) == 'string' and uid:match(CLIENT_UUID) then
    table.insert(out, '--tmux-client-uid')
    table.insert(out, uid)
  end
  return out
end

local function callback(verb, args)
  return wezterm.action_callback(function(window, pane)
    controller.run(window, pane, verb, args)
  end)
end

local function direction_action(verb, direction, extra)
  local args = { '--direction', direction }
  for _, value in ipairs(extra or {}) do
    table.insert(args, value)
  end
  return callback(verb, args)
end

function M.new_space_prompt(dir)
  return act.PromptInputLine {
    description = 'New dmux Space (inherits active owner and backend)',
    action = wezterm.action_callback(function(window, pane, name)
      if name and #name > 0 then
        local args = { '--name', name }
        if dir then
          table.insert(args, '--dir')
          table.insert(args, dir)
        end
        controller.run(window, pane, 'space-new', M.with_tmux_client_uid(pane, args))
      end
    end),
  }
end

local function rename_group()
  return act.PromptInputLine {
    description = 'Rename logical Group',
    action = wezterm.action_callback(function(window, pane, name)
      if name and #name > 0 then
        controller.run(window, pane, 'group-rename', { '--name', name })
      end
    end),
  }
end

local function same_prompt_context(expected, current, include_split)
  if type(expected) ~= 'table' or type(current) ~= 'table' then
    return false
  end
  for _, field in ipairs { 'host_uid', 'space_uid', 'space_no', 'backend', 'domain', 'server_epoch', 'group_ref' } do
    if expected[field] ~= current[field] then
      return false
    end
  end
  return not include_split or expected.split_ref == current.split_ref
end

local function prompt_context_is_current(window, pane, expected, include_split)
  local current = controller.run(window, pane, 'context', { '--cache' })
  if current and same_prompt_context(expected.marker, current.marker, include_split) then
    local expected_display = expected.display
    local current_display = current.display
    if type(expected_display) == 'table' and type(current_display) == 'table' then
      local same_display = true
      for _, field in ipairs { 'logical_ref', 'space_name', 'group_name', 'group_count', 'split_count' } do
        if expected_display[field] ~= current_display[field] then
          same_display = false
          break
        end
      end
      if same_display then
        return true
      end
    end
  end
  controller.toast(window, 'dmux context changed while the confirmation was open; nothing was removed')
  return false
end

local function close_group()
  return wezterm.action_callback(function(window, pane)
    local record = controller.run(window, pane, 'context', { '--cache' })
    if not record or type(record.display) ~= 'table' then
      return
    end
    local display = record.display
    local group_name = display.group_name and (display.group_name .. ' (' .. record.marker.group_ref .. ')')
      or record.marker.group_ref
    window:perform_action(
      act.InputSelector {
        title = string.format(
          'Remove Group %s from Space %s (%s)?',
          group_name,
          display.space_name,
          display.logical_ref
        ),
        choices = {
          { id = 'cancel', label = 'Cancel (no changes)' },
          { id = 'remove', label = 'Remove this Group' },
        },
        action = wezterm.action_callback(function(inner_window, inner_pane, id)
          if id ~= 'remove' then
            return
          end
          if not prompt_context_is_current(inner_window, inner_pane, record, false) then
            return
          end
          if display.group_count == 1 then
            inner_window:perform_action(
              act.InputSelector {
                title = string.format(
                  'This is the final Group. Remove Space %s (%s)?',
                  display.space_name,
                  display.logical_ref
                ),
                choices = {
                  { id = 'cancel', label = 'Cancel (keep the Space)' },
                  { id = 'remove-space', label = 'Remove the entire Space' },
                },
                action = wezterm.action_callback(function(final_window, final_pane, final_id)
                  if
                    final_id == 'remove-space'
                    and prompt_context_is_current(final_window, final_pane, record, false)
                  then
                    controller.run(final_window, final_pane, 'group-remove', { '--confirmed', '--escalate-space' })
                  end
                end),
              },
              inner_pane
            )
          else
            controller.run(inner_window, inner_pane, 'group-remove', { '--confirmed' })
          end
        end),
      },
      pane
    )
  end)
end

local function close_split()
  return wezterm.action_callback(function(window, pane)
    local record = controller.run(window, pane, 'context', { '--cache' })
    if not record or type(record.display) ~= 'table' then
      return
    end
    window:perform_action(
      act.InputSelector {
        title = string.format(
          'Remove Split %s from %s (%s)?',
          record.marker.split_ref,
          record.display.space_name,
          record.display.logical_ref
        ),
        choices = {
          { id = 'cancel', label = 'Cancel (no changes)' },
          { id = 'remove', label = 'Remove this Split' },
        },
        action = wezterm.action_callback(function(inner_window, inner_pane, id)
          if id == 'remove' and prompt_context_is_current(inner_window, inner_pane, record, true) then
            controller.run(inner_window, inner_pane, 'split-remove', { '--confirmed' })
          end
        end),
      },
      pane
    )
  end)
end

function M.safe_quit()
  return callback 'safe-quit'
end

function M.disconnect(domain)
  return callback('disconnect', domain and { '--domain' } or nil)
end

function M.keys()
  local keys = {
    -- Space/Group creation. Nothing here calls a native Spawn action.
    { key = 't', mods = 'CTRL|SHIFT', action = callback 'group-new' },
    { key = 'n', mods = 'LEADER', action = M.new_space_prompt() },

    -- Group navigation, selection, rename, and exact confirmed removal.
    { key = 'Tab', mods = 'CTRL', action = callback('group-select', { '--relative', 'next' }) },
    { key = 'Tab', mods = 'CTRL|SHIFT', action = callback('group-select', { '--relative', 'prev' }) },
    { key = 'n', mods = 'LEADER|SHIFT', action = callback('group-select', { '--relative', 'next' }) },
    { key = 'p', mods = 'LEADER|SHIFT', action = callback('group-select', { '--relative', 'prev' }) },
    { key = ',', mods = 'LEADER', action = rename_group() },
    { key = 'w', mods = 'CTRL', action = close_group() },

    -- Split creation and logical operations.
    { key = 'phys:Backslash', mods = 'LEADER', action = direction_action('split-new', 'right') },
    { key = 'phys:Minus', mods = 'LEADER', action = direction_action('split-new', 'down') },
    { key = 'phys:8', mods = 'CTRL|SHIFT', action = direction_action('split-new', 'right') },
    { key = 'phys:9', mods = 'CTRL|SHIFT', action = direction_action('split-new', 'down') },
    { key = 'h', mods = 'LEADER', action = direction_action('split-select', 'left') },
    { key = 'j', mods = 'LEADER', action = direction_action('split-select', 'down') },
    { key = 'k', mods = 'LEADER', action = direction_action('split-select', 'up') },
    { key = 'l', mods = 'LEADER', action = direction_action('split-select', 'right') },
    { key = 'z', mods = 'LEADER', action = callback 'split-zoom' },
    { key = 'x', mods = 'LEADER', action = close_split() },
    { key = 'r', mods = 'LEADER', action = act.ActivateKeyTable { name = 'dmux_resize_split', one_shot = false } },

    -- Domain disconnect is owner-aware and non-destructive.
    { key = 'd', mods = 'LEADER', action = M.disconnect(false) },
    { key = 'D', mods = 'LEADER', action = M.disconnect(true) },
    -- Linux/desktop window-close chord follows the same proved lifecycle as
    -- Alt+F4; Ctrl+W remains the logical Group close action above.
    { key = 'w', mods = 'CTRL|SHIFT', action = M.safe_quit() },
    { key = 'F4', mods = 'ALT', action = M.safe_quit() },
  }

  for index = 1, 9 do
    table.insert(
      keys,
      { key = tostring(index), mods = 'CTRL', action = callback('group-select', { '--index', tostring(index) }) }
    )
  end
  table.insert(keys, { key = '0', mods = 'CTRL', action = callback('group-select', { '--relative', 'last' }) })
  return keys
end

function M.mac_keys()
  local keys = {
    { key = 'q', mods = 'CMD', action = M.safe_quit() },
    { key = 'n', mods = 'CMD', action = M.new_space_prompt() },
    { key = 't', mods = 'CMD', action = callback 'group-new' },
    { key = 't', mods = 'CMD|SHIFT', action = M.new_space_prompt() },
    { key = 'w', mods = 'CMD', action = close_group() },
    { key = 'd', mods = 'CMD', action = direction_action('split-new', 'right') },
    { key = 'd', mods = 'CMD|SHIFT', action = direction_action('split-new', 'down') },
  }
  for index = 1, 8 do
    table.insert(
      keys,
      { key = tostring(index), mods = 'CMD', action = callback('group-select', { '--index', tostring(index) }) }
    )
  end
  table.insert(keys, { key = '9', mods = 'CMD', action = callback('group-select', { '--relative', 'last' }) })
  return keys
end

function M.key_tables()
  return {
    dmux_resize_split = {
      { key = 'h', action = direction_action('split-resize', 'left', { '--amount', '3' }) },
      { key = 'j', action = direction_action('split-resize', 'down', { '--amount', '3' }) },
      { key = 'k', action = direction_action('split-resize', 'up', { '--amount', '3' }) },
      { key = 'l', action = direction_action('split-resize', 'right', { '--amount', '3' }) },
      { key = 'Escape', action = act.PopKeyTable },
      { key = 'Enter', action = act.PopKeyTable },
    },
  }
end

return M
