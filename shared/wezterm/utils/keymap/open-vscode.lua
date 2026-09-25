local wezterm = require "wezterm"
local platform = require "utils.platform"
local host = require "domain.hosts" ---@type Hosts

---@param cwd Url
---@param pane Pane
---@return boolean
local function open_remote(cwd, pane)
  if not platform.is_remote(pane) then
    return false
  end

  ---@type string
  local hostname = host.target.hostname
  assert(type(hostname) == "string")
  local authority = "ssh-remote+" .. hostname
  local command = ("code --remote %s %s"):format(
    wezterm.shell_quote_arg(authority),
    wezterm.shell_quote_arg(cwd.file_path)
  )

  wezterm.background_child_process {
    "/bin/zsh",
    "-lic",
    command,
  }

  return true
end

local open_vscode = wezterm.action_callback(function(_, pane)
  local cwd = pane:get_current_working_dir()

  if not cwd or cwd.scheme ~= "file" then
    return
  end

  if open_remote(cwd, pane) then
    return
  elseif platform.is_mac then
    wezterm.background_child_process {
      "/usr/bin/open",
      "-a",
      "Visual Studio Code",
      cwd.file_path,
    }
  elseif platform.is_linux then
    local command = "cd " .. wezterm.shell_quote_arg(cwd.file_path) .. " && code ."

    wezterm.background_child_process {
      "/bin/zsh",
      "-lic",
      command,
    }
  end
end)

return open_vscode
