local wezterm = require 'wezterm'

local M = {}

function M.enabled()
  return os.getenv 'DMUX_WEZ_FIRST' == '1'
end

function M.preflight(config)
  if not M.enabled() then
    return
  end
  local runtime, runtime_err = require('wez.dmux_bridge.instance').runtime_dir()
  if not runtime then
    error('dmux bridge preflight: ' .. tostring(runtime_err))
  end
  local key, key_err = require('wez.dmux_bridge.fs').read(runtime .. '/bridge/key', 33)
  if not key or #key ~= 32 then
    error('dmux bridge preflight: raw 32-byte broker key unavailable: ' .. tostring(key_err))
  end
  -- Mandatory and first in flag-on config evaluation: a later module may
  -- fail, but stock/default key actions can never reappear in the interim.
  config.disable_default_key_bindings = true
  config.disable_default_mouse_bindings = true
  -- Maintained-fork primitive: removes built-in menu/palette/launcher/Dock
  -- creation and lifecycle bypasses, refuses native terminate/open/new, and
  -- turns OS window close into a broker event instead of mux.kill_window.
  config.dmux_managed_gui = true
  config.keys = {}
  config.key_tables = {}
  config.mouse_bindings = {}
  config.launch_menu = {}
  -- The native/integrated close control routes directly to mux.kill_window
  -- and has no Lua interception hook. Plan §13.4 therefore requires the
  -- fail-safe branch: remove the control entirely in managed mode.
  config.window_decorations = 'RESIZE'
  config.window_close_confirmation = 'NeverPrompt'
  config.quit_when_all_windows_are_closed = false
  config.show_new_tab_button_in_tab_bar = false
  config.show_close_tab_button_in_tabs = false
end

local function append_keys(destination, module_name)
  local source = require(module_name)
  for _, key in ipairs(source.keys or {}) do
    table.insert(destination, key)
  end
end

local function publish_persistent_domains(config)
  local names, seen = {}, {}
  for _, field in ipairs { 'unix_domains', 'ssh_domains' } do
    local entries = config[field] or {}
    if type(entries) ~= 'table' then
      error('dmux bridge: config.' .. field .. ' must be an array')
    end
    for index, entry in ipairs(entries) do
      if
        type(entry) ~= 'table'
        or type(entry.name) ~= 'string'
        or #entry.name == 0
        or #entry.name > 128
        or entry.name == 'local'
        or entry.name:find '[%z\1-\31\127]'
      then
        error(string.format('dmux bridge: config.%s[%d] has an invalid persistent domain name', field, index))
      end
      if seen[entry.name] then
        error('dmux bridge: duplicate configured persistent domain: ' .. entry.name)
      end
      seen[entry.name] = true
      table.insert(names, entry.name)
    end
    for key in pairs(entries) do
      if type(key) ~= 'number' or key < 1 or key % 1 ~= 0 or key > #entries then
        error('dmux bridge: config.' .. field .. ' must be a dense array')
      end
    end
  end
  table.sort(names)
  -- Config evaluation and bridge callbacks share the process-global table.
  -- Publish only after the final sanitizer has observed the exact compatible
  -- domain configuration; consumer startup fails closed if this is absent.
  wezterm.GLOBAL.dmux_managed_persistent_domains = names
end

function M.apply(config)
  if not M.enabled() then
    return
  end
  local actions = require 'wez.dmux_bridge.actions'
  local picker = require 'wez.plugins.workspace_picker'
  local platform = require 'wez.platform'
  local safe = {}
  -- Rebuild presentation-neutral keys from mandatory in-repo modules. A
  -- chord allowlist over `config.keys` would preserve an unsafe action if a
  -- later plugin replaced the action while keeping the same chord.
  append_keys(safe, 'wez.keys.leader')
  append_keys(safe, 'wez.keys.copy')
  append_keys(safe, 'wez.keys.window')
  append_keys(safe, 'wez.keys.mac')
  for _, key in ipairs(actions.keys()) do
    table.insert(safe, key)
  end
  if platform.is_mac then
    for _, key in ipairs(actions.mac_keys()) do
      table.insert(safe, key)
    end
  end
  table.insert(safe, { key = 'w', mods = 'LEADER', action = picker.action() })
  config.keys = safe

  -- Discard every plugin-generated/native table and install only dmux's
  -- validated resize path. Opaque plugin callbacks cannot cross this final
  -- managed-mode safety boundary merely by using an allowlisted chord.
  local tables = {}
  for name, entries in pairs(actions.key_tables()) do
    tables[name] = entries
  end
  config.key_tables = tables
  config.launch_menu = {}
  M.preflight(config)
  -- preflight clears keys/tables by design; restore the sanitized products.
  config.keys = safe
  config.key_tables = tables
  config.mouse_bindings = {}
  publish_persistent_domains(config)
end

function M.setup()
  if not M.enabled() then
    return
  end
  if wezterm.GLOBAL.dmux_bridge_events_registered then
    return
  end
  wezterm.GLOBAL.dmux_bridge_events_registered = true
  wezterm.on('dmux-managed-window-close-requested', function(window, pane)
    if wezterm.GLOBAL.dmux_managed_close_in_progress then
      return
    end
    wezterm.GLOBAL.dmux_managed_close_in_progress = true
    local ok, err = pcall(function()
      return require('wez.dmux_bridge.controller').run(window, pane, 'safe-quit')
    end)
    wezterm.GLOBAL.dmux_managed_close_in_progress = false
    if not ok then
      wezterm.log_error('dmux managed close failed closed: ' .. tostring(err))
    end
  end)
  local function start_guarded()
    local ok, err = pcall(function()
      return require('wez.dmux_bridge.consumer').start()
    end)
    if not ok then
      wezterm.log_error('dmux bridge startup failed: ' .. tostring(err))
    end
  end
  wezterm.on('gui-startup', start_guarded)
  wezterm.on('gui-attached', start_guarded)
end

return M
