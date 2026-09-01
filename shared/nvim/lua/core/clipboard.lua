local is_ssh = vim.env.SSH_CONNECTION ~= nil or vim.env.SSH_TTY ~= nil
local has_native_clipboard = vim.fn.has "mac" == 1 or vim.env.WAYLAND_DISPLAY ~= nil or vim.env.DISPLAY ~= nil

if is_ssh or not has_native_clipboard then
  local osc52 = require "vim.ui.clipboard.osc52"
  local last_copy_file = vim.fn.stdpath "cache" .. "/osc52-clipboard"

  local function copy(register)
    local send = osc52.copy(register)
    return function(lines)
      pcall(vim.fn.writefile, lines, last_copy_file)
      send(lines)
    end
  end

  local function paste_last_copy()
    if vim.fn.filereadable(last_copy_file) == 0 then
      return {}
    end
    return vim.fn.readfile(last_copy_file)
  end

  vim.g.clipboard = {
    name = "osc52",
    copy = { ["+"] = copy "+", ["*"] = copy "*" },
    paste = { ["+"] = paste_last_copy, ["*"] = paste_last_copy },
    cache_enabled = true,
  }
end

vim.o.clipboard = "unnamedplus"
