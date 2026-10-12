local wezterm = require "wezterm"
local platform = require "utils.platform"
local host = require "domain.hosts" ---@type Hosts

---@param path string
---@param pane Pane
---@return boolean
local function open_remote(path, pane)
  if not platform.is_remote(pane) then
    return false
  end

  ---@type string
  local hostname = host.target.hostname
  assert(type(hostname) == "string")
  local authority = "ssh-remote+" .. hostname

  if path:sub(-1) ~= "/" then
    path = path .. "/"
  end

  local command = ("code --remote %s %s"):format(wezterm.shell_quote_arg(authority), wezterm.shell_quote_arg(path))

  wezterm.background_child_process {
    "/bin/zsh",
    "-lic",
    command,
  }

  return true
end

---@param path string
---@param pane Pane
local function open(path, pane)
  if open_remote(path, pane) then
    return
  elseif platform.is_mac then
    wezterm.background_child_process {
      "/usr/bin/open",
      "-a",
      "Visual Studio Code",
      path,
    }
  elseif platform.is_linux then
    local command = "cd " .. wezterm.shell_quote_arg(path) .. " && code ."

    wezterm.background_child_process {
      "/bin/zsh",
      "-lic",
      command,
    }
  end
end

local open_cwd = wezterm.action_callback(function(_, pane)
  local cwd = pane:get_current_working_dir()

  if not cwd or cwd.scheme ~= "file" or not cwd.file_path then
    return
  end

  open(cwd.file_path, pane)
end)

return {
  cwd = open_cwd,
  open = open,
}
