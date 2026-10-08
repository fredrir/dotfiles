local wezterm = require "wezterm"
local remote = require "utils.remote"
local trim = require("utils.str").trim

return wezterm.action_callback(function(window, pane)
  local cwd = pane:get_current_working_dir()
  if not cwd or cwd.scheme ~= "file" then
    return
  end

  local args = { "git", "-C", cwd.file_path, "add", "--", "." }
  if remote.is_remote(pane) then
    local target = remote.ssh_target(window, pane)
    if not target then
      return
    end
    args = {
      "ssh",
      "-nT",
      "-o",
      "BatchMode=yes",
      "-o",
      "ConnectTimeout=5",
      "--",
      target,
      "git -C " .. wezterm.shell_quote_arg(cwd.file_path) .. " add -- .",
    }
  end

  local ran, ok, _, stderr = pcall(wezterm.run_child_process, args)
  if not ran then
    notify(window, tostring(ok))
  elseif not ok then
    local message = trim(stderr)
    notify(window, message ~= "" and message or "Failed to stage changes")
  end
end)