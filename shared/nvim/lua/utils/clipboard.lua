local M = {}

-- OSC 52 clipboard for SSH sessions without a native clipboard provider.
function M.setup()
  local is_ssh = vim.env.SSH_CONNECTION ~= nil or vim.env.SSH_TTY ~= nil
  local has_native_clipboard = vim.fn.has "mac" == 1 or vim.env.WAYLAND_DISPLAY ~= nil or vim.env.DISPLAY ~= nil
  if not is_ssh and has_native_clipboard then
    return
  end

  local cache_path = vim.fn.stdpath "cache" .. "/osc52-clipboard"

  ---@param register "+"|"*"
  local function copy(register)
    local send = require("vim.ui.clipboard.osc52").copy(register)
    return function(lines)
      pcall(vim.fn.writefile, lines, cache_path)
      send(lines)
    end
  end

  local function paste()
    if vim.fn.filereadable(cache_path) == 0 then
      return {}
    end
    return vim.fn.readfile(cache_path)
  end

  vim.g.clipboard = {
    name = "osc52",
    copy = { ["+"] = copy "+", ["*"] = copy "*" },
    paste = { ["+"] = paste, ["*"] = paste },
    cache_enabled = true,
  }
end

return M
