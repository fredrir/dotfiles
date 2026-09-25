local wezterm = require "wezterm"
local host = require "domain.hosts" ---@type Hosts"

local local_domains = {
  ["local"] = true,
  localmux = true,
}

local remote_programs = {
  ssh = true,
  mosh = true,
  ["mosh-client"] = true,
  rlogin = true,
  telnet = true,
}

---@param hostname string?
---@return string?
local function normalize_hostname(hostname)
  if not hostname or hostname == "" then
    return nil
  end

  local normalized = hostname:lower():gsub("%.$", "")
  if normalized:find(":", 1, true) or normalized:match "^%d+%.%d+%.%d+%.%d+$" then
    return normalized
  end

  return normalized:match "^[^.]+"
end

local local_hostnames = {
  localhost = true,
  ["127.0.0.1"] = true,
  ["::1"] = true,
}
local local_hostname = normalize_hostname(wezterm.hostname())
if local_hostname then
  local_hostnames[local_hostname] = true
end

---@param hostname string?
---@return boolean
local function is_remote_hostname(hostname)
  local normalized = normalize_hostname(hostname)
  return normalized ~= nil and not local_hostnames[normalized]
end

---@param command string?
---@return boolean
local function is_remote_program(command)
  if not command or command == "" then
    return false
  end

  ---@type string?
  local executable = command:match "^%s*([^%s]+)"
  if not executable then
    return false
  end

  ---@type string?
  local name = executable:match "([^/\\]+)$"
  if not name then
    return false
  end

  name = name:lower():gsub("%.exe$", "")
  return remote_programs[name] == true
end

---@param pane Pane
---@return boolean
local function is_remote(pane)
  local domain = pane:get_domain_name()
  if domain and domain ~= "" and not local_domains[domain] then
    return true
  end

  local user_vars = pane:get_user_vars()
  if is_remote_hostname(user_vars.WEZTERM_HOST) or is_remote_program(user_vars.WEZTERM_PROG) then
    return true
  end

  local cwd = pane:get_current_working_dir()
  if cwd ~= nil and is_remote_hostname(cwd.host) then
    return true
  end

  return is_remote_program(pane:get_foreground_process_name())
end

-- ssh(1)/mosh(1) options that consume the following argument
local value_options = {
  B = true,
  c = true,
  D = true,
  E = true,
  e = true,
  F = true,
  I = true,
  i = true,
  J = true,
  L = true,
  l = true,
  m = true,
  O = true,
  o = true,
  P = true,
  p = true,
  Q = true,
  R = true,
  S = true,
  W = true,
  w = true,
  ssh = true,
  port = true,
  predict = true,
}

---@param command string?
---@return string?
local function command_target(command)
  ---@type string[]
  local tokens = {}
  for token in (command or ""):gmatch "%S+" do
    table.insert(tokens, token)
  end

  local executable = tokens[1] and tokens[1]:match "([^/\\]+)$"
  if not executable or not remote_programs[executable:lower():gsub("%.exe$", "")] then
    return nil
  end

  local skip = false
  for index = 2, #tokens do
    local token = tokens[index]
    if skip then
      skip = false
    elseif token:sub(1, 1) == "-" then
      local option = token:match "^%-%-([^=]+)" or token:match "^%-(.)"
      local attached = token:find("=", 1, true) ~= nil or (token:sub(1, 2) ~= "--" and #token > 2)
      if option and value_options[option] and not attached then
        skip = true
      end
    else
      return token:match "[^@]+$"
    end
  end
end

---@param pane Pane
---@return string?
local function target(pane)
  local user_vars = pane:get_user_vars()

  if is_remote_hostname(user_vars.WEZTERM_HOST) then
    return command_target(user_vars.WEZTERM_PROG) or user_vars.WEZTERM_HOST
  end

  local cwd = pane:get_current_working_dir()
  if cwd ~= nil and is_remote_hostname(cwd.host) then
    return cwd.host
  end

  return command_target(user_vars.WEZTERM_PROG)
end

---@param window Window
---@param pane Pane
---@return string?
local function ssh_target(window, pane)
  local name = pane:get_domain_name()

  if name and name ~= "" then
    local config = window:effective_config()

    for _, domain in ipairs(config.ssh_domains or {}) do
      if domain.name == name then
        return name
      end
    end

    for _, domain in ipairs(config.unix_domains or {}) do
      if domain.name == name and domain.proxy_command then
        return name
      end
    end

    for _, domain in ipairs(config.tls_clients or {}) do
      if domain.name == name then
        return host.target.hostname
      end
    end
  end

  local platform = require "utils.platform" -- deferred: utils.platform requires utils.remote
  return platform.remote_target(pane)
end

return {
  is_remote = is_remote,
  target = target,
  ssh_target = ssh_target,
}
