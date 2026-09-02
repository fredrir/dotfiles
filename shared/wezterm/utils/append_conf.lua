local wezterm = require "wezterm"

local INIT_GLOB = "*/init.lua"
local EXISTING_CONFIG = "the existing configuration"
local MAX_PROVIDER_DEPTH = 64

---@class ConfigMergeState
---@field owners table<string, string>
---@field active_modules table<string, boolean>
---@field active_values table<table|function, string>

---@param value any
---@return string
local function value_type(value)
  local kind = type(value)
  if kind == "table" then
    return "table"
  end
  return kind
end

---@param message string
---@param ... any
local function fail(message, ...)
  error(("append_conf: " .. message):format(...), 0)
end

---@param config Config
local function validate_target(config)
  if type(config) ~= "table" then
    fail("expected a configuration table, got %s", value_type(config))
  end

  local dynamic_config = config --[[@as table<any, any>]]
  for key in pairs(dynamic_config) do
    if type(key) ~= "string" then
      fail("the configuration contains a non-string key of type %s", value_type(key))
    end
  end
end

---@param config Config
---@return ConfigMergeState
local function new_state(config)
  ---@type table<string, string>
  local owners = {}
  local dynamic_config = config --[[@as table<string, any>]]
  for key in pairs(dynamic_config) do
    owners[key] = EXISTING_CONFIG
  end

  return {
    owners = owners,
    active_modules = {},
    active_values = {},
  }
end

---@param config Config
---@param key string
---@param value any
local function assign_option(config, key, value)
  ---The provider boundary is intentionally dynamic. Assignment must go through
  ---the ConfigBuilder metatable so WezTerm can validate the option and value.
  local dynamic_config = config --[[@as table<string, any>]]
  dynamic_config[key] = value
end

---@param config Config
---@param fragment ConfigFragment
---@param source string
---@param state ConfigMergeState
local function apply_fragment(config, fragment, source, state)
  ---@type string[]
  local keys = {}
  local dynamic_fragment = fragment --[[@as table<any, any>]]

  for key in pairs(dynamic_fragment) do
    if type(key) ~= "string" then
      fail("provider %s returned a fragment with a non-string key of type %s", source, value_type(key))
    end
    table.insert(keys, key)
  end

  table.sort(keys)

  for _, key in ipairs(keys) do
    local previous_source = state.owners[key]
    if previous_source then
      fail("provider %s conflicts on option %q, which was already set by %s", source, key, previous_source)
    end

    local value = dynamic_fragment[key]
    local ok, assign_error = pcall(assign_option, config, key, value)
    if not ok then
      fail("provider %s failed while setting option %q:\n%s", source, key, tostring(assign_error))
    end

    state.owners[key] = source
  end
end

---@param config Config
---@return table<string, any>, table<string, boolean>
local function snapshot_config(config)
  ---@type table<string, any>
  local values = {}
  ---@type table<string, boolean>
  local present = {}
  local dynamic_config = config --[[@as table<any, any>]]

  for key, value in pairs(dynamic_config) do
    if type(key) ~= "string" then
      fail("the configuration contains a non-string key of type %s", value_type(key))
    end
    values[key] = value
    present[key] = true
  end

  return values, present
end

---@param config Config
---@param before table<string, any>
---@param was_present table<string, boolean>
---@param source string
---@param state ConfigMergeState
local function record_mutations(config, before, was_present, source, state)
  local current = config --[[@as table<string, any>]]

  for key, old_value in pairs(before) do
    local new_value = rawget(current, key)
    if new_value == nil then
      fail("provider %s removed option %q, which was set by %s", source, key, state.owners[key] or EXISTING_CONFIG)
    end
    if not rawequal(old_value, new_value) then
      fail("provider %s overwrote option %q, which was set by %s", source, key, state.owners[key] or EXISTING_CONFIG)
    end
  end

  for key in pairs(current) do
    if type(key) ~= "string" then
      fail("provider %s added a non-string configuration key of type %s", source, value_type(key))
    end
    if not was_present[key] then
      state.owners[key] = source
    end
  end
end

---@param err any
---@return string
local function with_traceback(err)
  local message = tostring(err)
  if type(debug) == "table" and type(debug.traceback) == "function" then
    local ok, traceback = pcall(debug.traceback, message, 2)
    if ok then
      return traceback
    end
  end
  return message
end

---@type fun(config: Config, provider: any, source: string, state: ConfigMergeState, depth: integer)
local apply_provider

---@param config Config
---@param provider ConfigProviderFunction
---@param source string
---@param state ConfigMergeState
---@param depth integer
local function apply_function(config, provider, source, state, depth)
  local before, was_present = snapshot_config(config)
  local returned_fragment ---@type ConfigFragment?

  local ok, provider_error = xpcall(function()
    returned_fragment = provider(config)
  end, with_traceback)

  if not ok then
    fail("provider %s failed:\n%s", source, tostring(provider_error))
  end

  record_mutations(config, before, was_present, source, state)

  if returned_fragment ~= nil and returned_fragment ~= config then
    apply_provider(config, returned_fragment, source .. " return value", state, depth + 1)
  end
end

---@param module_name string
---@return any
local function load_module(module_name)
  if module_name == "" then
    fail "module names must not be empty"
  end
  if module_name:find "[/\\%z]" then
    fail("module name %q must use Lua dot notation rather than a file path", module_name)
  end

  local ok, provider = pcall(require, module_name)
  if not ok then
    fail("failed to load module %q:\n%s", module_name, tostring(provider))
  end
  return provider
end

---@param config Config
---@param provider any
---@param source string
---@param state ConfigMergeState
---@param depth integer
apply_provider = function(config, provider, source, state, depth)
  if depth > MAX_PROVIDER_DEPTH then
    fail("provider nesting exceeded %d levels while applying %s", MAX_PROVIDER_DEPTH, source)
  end

  local kind = type(provider)

  if kind == "string" then
    local module_name = provider --[[@as string]]
    if state.active_modules[module_name] then
      fail("cyclic module provider detected while loading %q", module_name)
    end

    state.active_modules[module_name] = true
    local loaded = load_module(module_name)
    apply_provider(config, loaded, ("module %q"):format(module_name), state, depth + 1)
    state.active_modules[module_name] = nil
    return
  end

  if kind ~= "table" and kind ~= "function" then
    fail("provider %s must be a module name, table, or function; got %s", source, value_type(provider))
  end

  local identity = provider --[[@as table|function]]
  local active_source = state.active_values[identity]
  if active_source then
    fail("cyclic provider detected: %s is already being applied by %s", source, active_source)
  end
  state.active_values[identity] = source

  if kind == "function" then
    apply_function(config, provider --[[@as ConfigProviderFunction]], source, state, depth)
  else
    local fragment = provider --[[@as ConfigFragment]]
    local legacy_apply = rawget(fragment, "apply_to_config")
    if type(legacy_apply) == "function" then
      wezterm.log_warn(
        ("append_conf: provider %s uses legacy apply_to_config; return a configuration fragment or function instead"):format(
          source
        )
      )
      apply_function(config, legacy_apply --[[@as ConfigProviderFunction]], source .. ".apply_to_config", state, depth)
    else
      apply_fragment(config, fragment, source, state)
    end
  end

  state.active_values[identity] = nil
end

---@param path string
---@return string
local function normalize_path(path)
  local normalized = path:gsub("\\", "/")
  if normalized == "/" or normalized:match "^%a:/$" then
    return normalized
  end
  return (normalized:gsub("/+$", ""))
end

---@param directory string
---@param name string
---@return string
local function join_path(directory, name)
  if directory == "" or directory:sub(-1) == "/" then
    return directory .. name
  end
  return directory .. "/" .. name
end

---@return string[]
local function discover_modules()
  local config_dir = normalize_path(wezterm.config_dir)
  local pattern = join_path(config_dir, INIT_GLOB)
  local ok ---@type boolean
  local paths ---@type any

  if config_dir == "" then
    ok, paths = pcall(wezterm.glob, pattern)
  else
    ok, paths = pcall(wezterm.glob, pattern, config_dir)
  end

  if not ok then
    fail("failed to discover configuration providers with %q:\n%s", pattern, tostring(paths))
  end
  if type(paths) ~= "table" then
    fail("wezterm.glob returned %s instead of a path list", value_type(paths))
  end

  ---@type string[]
  local modules = {}
  ---@type table<string, boolean>
  local seen = {}
  ---@type any[]
  local path_list = paths

  for index, path in ipairs(path_list) do
    if type(path) ~= "string" then
      fail("discovery result %d is %s instead of a path string", index, value_type(path))
    end

    local relative_path = normalize_path(path)
    local prefix = config_dir == "" and nil or join_path(config_dir, "")
    if prefix and relative_path:sub(1, #prefix) == prefix then
      relative_path = relative_path:sub(#prefix + 1)
    end
    relative_path = relative_path:gsub("^%./", "")

    local module_name = relative_path:match "^([^/]+)/init%.lua$"
    if not module_name then
      fail("discovered path %q does not match the top-level */init.lua convention", path)
    end
    if seen[module_name] then
      fail("configuration provider %q was discovered more than once", module_name)
    end

    seen[module_name] = true
    table.insert(modules, module_name)
  end

  table.sort(modules)

  if #modules == 0 then
    wezterm.log_warn(("append_conf: no configuration providers matched %q"):format(pattern))
  end

  return modules
end

---Apply configuration providers to a ConfigBuilder or combine fragments into a table.
---With no providers, top-level `*/init.lua` modules are discovered automatically.
---@param config Config
---@param ... ConfigProvider
---@return Config
local function append_conf(config, ...)
  validate_target(config)

  local state = new_state(config)
  ---@type { n: integer, [integer]: ConfigProvider? }
  local providers = table.pack(...)

  if providers.n == 0 then
    local discovered = discover_modules()
    for index, module_name in ipairs(discovered) do
      providers[index] = module_name
    end
    providers.n = #discovered
  end

  for index = 1, providers.n do
    local provider = providers[index]
    if provider == nil then
      fail("provider %d is nil", index)
    end
    apply_provider(config, provider, ("provider %d"):format(index), state, 1)
  end

  return config
end

return append_conf
