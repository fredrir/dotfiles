local wezterm = require "wezterm" ---@type Wezterm
local platform = require "utils.platform"
local host = require "domain.hosts" ---@type Hosts

local SCRIPT = [[
root=$(git -C "$1" rev-parse --show-toplevel 2>/dev/null) || exit 1
cd "$root" || exit 1

if command -v gh >/dev/null 2>&1; then
  url=$(gh browse --no-browser 2>/dev/null)
  if [ -n "$url" ]; then
    printf 'gh %s\n' "$url"
    exit 0
  fi
fi

git remote get-url origin 2>/dev/null
for name in $(git remote); do
  if [ "$name" != origin ]; then
    git remote get-url "$name" 2>/dev/null
  fi
done

exit 0
]]

---@param text string
---@return string
local function trim(text)
  return (text:gsub("%s+$", ""))
end

---@param raw string
---@return string?
local function to_https(raw)
  local url = trim(raw)
  url = url:gsub("%.git$", "")
  url = url:gsub("^git@([^:]+):", "https://%1/")
  url = url:gsub("^ssh://git@", "https://")
  url = url:gsub("^git://", "https://")
  url = url:gsub("^(https?://)[^@/]+@", "%1")

  if url:match "^https?://" then
    return url
  end
end

---@param stdout string
---@return string?
local function parse(stdout)
  for line in stdout:gmatch "[^\r\n]+" do
    local browsed = line:match "^gh (.*)$"
    if browsed then
      return trim(browsed)
    end

    local url = to_https(line)
    if url and url:match "^https?://[^/]*github" then
      return url
    end
  end
end

---@param command string[]
---@return string?
local function run(command)
  local ok, stdout = wezterm.run_child_process(command)
  if ok then
    return parse(stdout)
  end
end

---@param command string
---@return string?
local function run_login(command)
  return run { "/bin/zsh", "-lc", command }
end

---@param path string
---@return string
local function command_for(path)
  return ("sh -c %s open-github %s"):format(wezterm.shell_quote_arg(SCRIPT), wezterm.shell_quote_arg(path))
end

---@param window Window
---@param pane Pane
---@return string?
local function ssh_target(window, pane)
  local name = pane:get_domain_name()
  if not name or name == "" then
    return
  end

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

---@param window Window
---@param pane Pane
---@param cwd Url
---@return string?
local function resolve(window, pane, cwd)
  local command = command_for(trim(cwd.file_path))

  if not platform.is_remote(pane) then
    return run_login(command)
  end

  local target = ssh_target(window, pane)
  if not target then
    return
  end

  return run_login(
    ("ssh -o BatchMode=yes %s %s"):format(wezterm.shell_quote_arg(target), wezterm.shell_quote_arg(command))
  )
end

local open_github = wezterm.action_callback(function(window, pane)
  local cwd = pane:get_current_working_dir()
  if not cwd or cwd.scheme ~= "file" then
    return
  end

  local url = resolve(window, pane, cwd)
  if url then
    wezterm.open_with(url)
  end
end)

return open_github
