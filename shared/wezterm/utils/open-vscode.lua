local wezterm = require "wezterm"
local platform = require "utils.platform"

local open_vscode = wezterm.action_callback(function(_, pane)
  local cwd = pane:get_current_working_dir()

  if not cwd or cwd.scheme ~= "file" then
    return
  end

  if platform.is_mac then
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
