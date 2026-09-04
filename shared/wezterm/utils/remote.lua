local wezterm = require "wezterm"

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

return is_remote
