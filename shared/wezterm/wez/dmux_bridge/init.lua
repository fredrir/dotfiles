local wezterm = require 'wezterm'

local M = {}

local UUID = '^[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]%-'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'
  .. '[0-9a-f][0-9a-f][0-9a-f][0-9a-f]$'

local DOMAIN_INSTANCE_PROVIDERS = {
  unix_domains = 'wez.domains',
  ssh_domains = 'wez.remote.mux',
}

function M.enabled()
  return os.getenv 'DMUX_WEZ_FIRST' == '1'
end

function M.preflight(config)
  if not M.enabled() then
    return
  end
  -- Config construction cannot acquire an instance lease because the new
  -- managed config is not effective yet. It can and must require the
  -- maintained-fork surface; consumer startup opens the exclusive lease and
  -- obtains the descriptor-backed key after this config becomes active.
  local proof, preflight_err = require('wez.dmux_bridge.instance').checked_preflight()
  if not proof then
    error('dmux bridge preflight: ' .. tostring(preflight_err))
  end
  -- Mandatory and first in flag-on config evaluation: a later module may
  -- fail, but stock/default key actions can never reappear in the interim.
  config.disable_default_key_bindings = true
  config.disable_default_mouse_bindings = true
  -- Maintained-fork primitive: removes built-in menu/palette/launcher/Dock
  -- creation and lifecycle bypasses, refuses native terminate/open/new, and
  -- turns OS window close into a broker event instead of mux.kill_window.
  config.dmux_managed_gui = true
  -- Managed bridge leases, request consumers, proof timers, and named event
  -- handlers are process-generation state. The maintained fork also blocks
  -- reload actions while managed; make the public config contract explicit.
  config.automatically_reload_config = false
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

local function configured_domain_facts(field)
  local provider_name = DOMAIN_INSTANCE_PROVIDERS[field]
  local provider = require(provider_name)
  if type(provider.managed_persistent_domain_instances) ~= 'function' then
    error('dmux bridge: ' .. provider_name .. ' did not publish managed domain identities')
  end
  local instances = provider.managed_persistent_domain_instances()
  if type(instances) ~= 'table' or getmetatable(instances) ~= nil then
    error('dmux bridge: ' .. provider_name .. ' managed domain identities are unavailable')
  end
  if field ~= 'ssh_domains' then
    return instances
  end
  if type(provider.managed_persistent_domain_owners) ~= 'function' then
    error('dmux bridge: ' .. provider_name .. ' did not publish managed domain owners')
  end
  local owners = provider.managed_persistent_domain_owners()
  if type(owners) ~= 'table' or getmetatable(owners) ~= nil then
    error('dmux bridge: ' .. provider_name .. ' managed domain owners are unavailable')
  end
  return instances, owners
end

local function publish_persistent_domains(config)
  local names, seen, published_instances = {}, {}, {}
  local local_instances, instance_by_owner, owner_by_instance = {}, {}, {}
  for _, field in ipairs { 'unix_domains', 'ssh_domains' } do
    local entries = config[field]
    if entries == nil then
      entries = {}
    end
    local instances, owners = configured_domain_facts(field)
    local field_names = {}
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
      field_names[entry.name] = true
      local backend_instance_uid = instances[entry.name]
      if type(backend_instance_uid) ~= 'string' or not backend_instance_uid:match(UUID) then
        error('dmux bridge: configured persistent domain has no canonical backend instance: ' .. entry.name)
      end
      if field == 'unix_domains' then
        local_instances[backend_instance_uid] = true
      else
        local host_uid = owners[entry.name]
        if type(host_uid) ~= 'string' or not host_uid:match(UUID) then
          error('dmux bridge: configured remote domain has no canonical owner: ' .. entry.name)
        end
        if local_instances[backend_instance_uid] then
          error('dmux bridge: remote domain aliases the local backend instance: ' .. entry.name)
        end
        if
          (instance_by_owner[host_uid] and instance_by_owner[host_uid] ~= backend_instance_uid)
          or (owner_by_instance[backend_instance_uid] and owner_by_instance[backend_instance_uid] ~= host_uid)
        then
          error('dmux bridge: remote owner/backend identity is not bijective: ' .. entry.name)
        end
        instance_by_owner[host_uid] = backend_instance_uid
        owner_by_instance[backend_instance_uid] = host_uid
      end
      published_instances[entry.name] = backend_instance_uid
      table.insert(names, entry.name)
    end
    for key in pairs(entries) do
      if type(key) ~= 'number' or key < 1 or key % 1 ~= 0 or key > #entries then
        error('dmux bridge: config.' .. field .. ' must be a dense array')
      end
    end
    for name, backend_instance_uid in pairs(instances) do
      if type(name) ~= 'string' or not field_names[name] then
        error('dmux bridge: ' .. field .. ' identity set does not match the final domain configuration')
      end
      if type(backend_instance_uid) ~= 'string' or not backend_instance_uid:match(UUID) then
        error('dmux bridge: configured persistent domain has a noncanonical backend instance: ' .. name)
      end
    end
    if owners then
      for name, host_uid in pairs(owners) do
        if type(name) ~= 'string' or not field_names[name] then
          error('dmux bridge: ' .. field .. ' owner set does not match the final domain configuration')
        end
        if type(host_uid) ~= 'string' or not host_uid:match(UUID) then
          error('dmux bridge: configured remote domain has a noncanonical owner: ' .. name)
        end
      end
    end
  end
  table.sort(names)
  -- Config evaluation and bridge callbacks share the process-global table.
  -- Publish only after the final sanitizer has observed the exact compatible
  -- domain configuration; consumer startup fails closed if this is absent.
  wezterm.GLOBAL.dmux_managed_persistent_domains = names
  wezterm.GLOBAL.dmux_managed_persistent_domain_instances = published_instances
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
  -- The maintained fork emits this with zero arguments when the resident
  -- application has no GUI pane from which to derive a marker. It must still
  -- traverse dmux's signed safe-quit protocol; failure deliberately leaves
  -- the application resident and never falls back to native quit.
  wezterm.on('dmux-managed-application-quit-requested', function()
    if wezterm.GLOBAL.dmux_managed_close_in_progress then
      return
    end
    wezterm.GLOBAL.dmux_managed_close_in_progress = true
    local ok, result, run_err = pcall(function()
      return require('wez.dmux_bridge.controller').run_resident 'safe-quit'
    end)
    wezterm.GLOBAL.dmux_managed_close_in_progress = false
    if not ok then
      wezterm.log_error('dmux managed application quit failed closed: ' .. tostring(result))
    elseif result == nil then
      wezterm.log_error('dmux managed application quit failed closed: ' .. tostring(run_err))
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
