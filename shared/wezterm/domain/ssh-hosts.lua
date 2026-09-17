local wezterm = require "wezterm" ---@type Wezterm
local host = require "domain.hosts"
local ssh_mux = require "domain.ssh-mux"

local SSH_DIR = wezterm.home_dir .. "/.ssh"

local mux_hosts = {
  [host.origin.hostname] = true,
  [host.target.hostname] = true,
}

---@param pattern string
---@return string[]
local function include_paths(pattern)
  pattern = pattern:gsub("^~", wezterm.home_dir)
  if pattern:sub(1, 1) ~= "/" then
    pattern = SSH_DIR .. "/" .. pattern
  end
  return wezterm.glob(pattern)
end

---@param path string
---@param hosts table<string, true>
local function collect(path, hosts)
  for line in io.lines(path) do
    local key, value = line:match "^%s*(%a+)[%s=]+(.-)%s*$"
    key = key and key:lower()
    if key == "include" then
      for pattern in value:gmatch "%S+" do
        for _, included in ipairs(include_paths(pattern)) do
          collect(included, hosts)
        end
      end
    elseif key == "host" then
      for name in value:gmatch "%S+" do
        if not name:find "[*?!]" then
          hosts[name] = true
        end
      end
    end
  end
end

local names ---@type string[]?

-- Not wezterm.enumerate_ssh_hosts: it resolves every host, which logs a warning per `Match exec` on every CLI call
---@return string[]
return function()
  if names then
    return names
  end

  local hosts = {}
  local ok, err = pcall(collect, SSH_DIR .. "/config", hosts)
  if not ok then
    wezterm.log_warn("ssh hosts: " .. tostring(err))
  end

  names = {}
  for name in pairs(hosts) do
    if not mux_hosts[name] and not ssh_mux.hosts[name] then
      table.insert(names, name)
    end
  end
  table.sort(names)
  return names
end
