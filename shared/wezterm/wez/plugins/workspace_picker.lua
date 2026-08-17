local wezterm = require 'wezterm'
local platform = require 'wez.platform'
local json = require 'wez.dmux_bridge.json'

-- Zoxide-driven workspace switcher: existing workspaces first, then zoxide
-- directories; picking a directory creates a workspace rooted there.
local M = {}

local URL = 'https://github.com/fredrir/workspace-picker.wezterm'

local function dmux_enabled()
  return os.getenv 'DMUX_WEZ_FIRST' == '1'
end

local function zoxide_path()
  return platform.pick {
    mac = '/opt/homebrew/bin/zoxide',
    default = '/usr/bin/zoxide',
  }
end

local function zoxide_dirs(window)
  local spawned, ok, stdout, stderr = pcall(wezterm.run_child_process, { zoxide_path(), 'query', '--list' })
  if not spawned or not ok then
    window:toast_notification('dmux', 'zoxide unavailable: ' .. tostring(spawned and stderr or ok), nil, 4000)
    return nil
  end
  local dirs, seen = {}, {}
  for line in stdout:gmatch '[^\r\n]+' do
    if line:sub(1, 1) == '/' and not seen[line] then
      seen[line] = true
      table.insert(dirs, line)
    end
  end
  return dirs
end

local function choose_zoxide(window, pane)
  local dirs = zoxide_dirs(window)
  if not dirs or #dirs == 0 then
    window:toast_notification('dmux', 'zoxide has no directories', nil, 4000)
    return
  end
  local choices = {}
  for _, dir in ipairs(dirs) do
    table.insert(choices, { id = dir, label = dir })
  end
  window:perform_action(
    wezterm.action.InputSelector {
      title = 'Create a dmux Space in a zoxide directory',
      choices = choices,
      fuzzy = true,
      action = wezterm.action_callback(function(inner_window, inner_pane, dir)
        if dir then
          inner_window:perform_action(require('wez.dmux_bridge.actions').new_space_prompt(dir), inner_pane)
        end
      end),
    },
    pane
  )
end

local SPACE_KEYS = {
  attached = true,
  backend = true,
  health = true,
  name = true,
  owner_alias = true,
  owner_label = true,
  ref = true,
  route = true,
}

local function bounded_string(value, maximum)
  return type(value) == 'string' and #value > 0 and #value <= maximum and not value:find '[%z\1-\31\127]'
end

local function valid_space(row)
  if type(row) ~= 'table' then
    return false
  end
  for key in pairs(row) do
    if type(key) ~= 'string' or not SPACE_KEYS[key] then
      return false
    end
  end
  return bounded_string(row.ref, 512)
    and bounded_string(row.name, 1024)
    -- `_gui spaces` is presentation inventory, not the general CLI list.
    -- Rust includes tmux rows only when the active pane's attach-time UID
    -- proves one exact invoking client on that same backend incarnation.
    and (row.backend == 'wez' or row.backend == 'tmux')
    and bounded_string(row.owner_alias, 64)
    and bounded_string(row.owner_label, 128)
    and bounded_string(row.route, 256)
    and type(row.attached) == 'boolean'
    and bounded_string(row.health, 128)
end

local function valid_spaces(rows)
  if type(rows) ~= 'table' or not json.is_array(rows) then
    return false
  end
  local refs = {}
  for index, row in ipairs(rows) do
    if not valid_space(row) or refs[row.ref] then
      return false
    end
    refs[row.ref] = true
  end
  for key in pairs(rows) do
    if type(key) ~= 'number' or key % 1 ~= 0 or key < 1 or key > #rows then
      return false
    end
  end
  return true
end

function M.action()
  if not dmux_enabled() then
    local picker = wezterm.plugin.require(URL)
    return wezterm.action_callback(function(window, pane)
      picker.show_workspace_selector(window, pane)
    end)
  end
  return wezterm.action_callback(function(window, pane)
    local controller = require 'wez.dmux_bridge.controller'
    local dmux_actions = require 'wez.dmux_bridge.actions'
    local result = controller.run(window, pane, 'spaces', dmux_actions.with_tmux_client_uid(pane))
    if not result or type(result.spaces) ~= 'table' then
      controller.report(window, 'spaces', 'dmux returned a malformed Space picker result')
      return
    end
    for key in pairs(result) do
      if key ~= 'spaces' then
        controller.report(window, 'spaces', 'dmux returned a malformed Space picker result')
        return
      end
    end
    if not valid_spaces(result.spaces) then
      controller.report(window, 'spaces', 'dmux returned malformed Space picker rows')
      return
    end
    local choices = {
      { id = '__new', label = '+ new Space (active owner/backend)' },
      { id = '__zoxide', label = '+ new Space from zoxide directory' },
    }
    for _, row in ipairs(result.spaces) do
      table.insert(choices, {
        id = row.ref,
        label = string.format(
          '%s  %s  · %s/%s · %s%s',
          row.ref,
          row.name,
          row.owner_label,
          row.backend,
          row.route,
          row.attached and ' · attached' or ''
        ),
      })
    end
    window:perform_action(
      wezterm.action.InputSelector {
        title = 'dmux Spaces',
        choices = choices,
        fuzzy = true,
        action = wezterm.action_callback(function(inner_window, inner_pane, id)
          if id == '__new' then
            inner_window:perform_action(require('wez.dmux_bridge.actions').new_space_prompt(), inner_pane)
          elseif id == '__zoxide' then
            choose_zoxide(inner_window, inner_pane)
          elseif id then
            controller.run(
              inner_window,
              inner_pane,
              'present',
              dmux_actions.with_tmux_client_uid(inner_pane, { '--space', id })
            )
          end
        end),
      },
      pane
    )
  end)
end

function M.apply(config)
  if dmux_enabled() then
    local keys = config.keys or {}
    table.insert(keys, { key = 'w', mods = 'LEADER', action = M.action() })
    config.keys = keys
    return
  end
  local picker = wezterm.plugin.require(URL)

  picker.setup {
    zoxide_path = platform.pick {
      mac = '/opt/homebrew/bin/zoxide',
      default = '/usr/bin/zoxide',
    },
    -- false, not nil: a nil field is indistinguishable from absent, so the
    -- plugin would re-install its default LEADER+s binding over the tmux
    -- session picker.
    keybinds = false,
  }

  local keys = config.keys or {}
  table.insert(keys, {
    key = 'w',
    mods = 'LEADER',
    action = wezterm.action_callback(function(window, pane)
      picker.show_workspace_selector(window, pane)
    end),
  })
  config.keys = keys
end

return M
