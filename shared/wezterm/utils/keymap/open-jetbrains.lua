local wezterm = require "wezterm"
local platform = require "utils.platform"
local remote = require "utils.remote"

---@param value string
---@return string
local function encode(value)
  return (value:gsub("([^%w%-._~])", function(char)
    return ("%%%02X"):format(string.byte(char))
  end))
end

---@param window Window
---@param pane Pane
---@param cwd Url
---@param product string
local function open_remote(window, pane, cwd, product)
  local target = remote.ssh_target(window, pane)
  if not target then
    wezterm.log_error "JetBrains: SSH target not found"
    return
  end

  local ok, stdout = wezterm.run_child_process { "ssh", "-G", "--", target }
  if not ok then
    wezterm.log_error("JetBrains: SSH config unavailable for " .. target)
    return
  end

  ---@type string?, string?
  local user, port
  for line in stdout:gmatch "[^\r\n]+" do
    user = user or line:match "^user%s+(.+)$"
    port = port or line:match "^port%s+(%d+)$"
  end
  if not user or not port then
    wezterm.log_error("JetBrains: SSH user or port missing for " .. target)
    return
  end

  local url = ("jetbrains://gateway/ssh/environment?h=%s&u=%s&p=%s&launchIde=true&ideHint=%s&projectHint=%s"):format(
    encode(target),
    encode(user),
    port,
    product,
    encode(cwd.file_path)
  )
  wezterm.open_with(url)
end

---@param app string
---@param command string
---@param product string
---@return Action
local function action(app, command, product)
  return wezterm.action_callback(function(window, pane)
    local cwd = pane:get_current_working_dir()
    if not cwd or cwd.scheme ~= "file" then
      return
    end

    if platform.is_remote(pane) then
      open_remote(window, pane, cwd, product)
    elseif platform.is_mac then
      wezterm.background_child_process { "/usr/bin/open", "-a", app, cwd.file_path }
    elseif platform.is_linux then
      wezterm.background_child_process {
        "/bin/zsh",
        "-lic",
        command .. " " .. wezterm.shell_quote_arg(cwd.file_path),
      }
    end
  end)
end

return {
  pycharm = action("PyCharm", "pycharm", "PY-262.10968.92"),
  rustrover = action("RustRover", "rustrover", "RR"),
  intellij = action("IntelliJ IDEA", "idea", "IU"),
}