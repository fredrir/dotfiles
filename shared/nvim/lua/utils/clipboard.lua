local M = {}

function M.setup()
  if vim.fn.executable "dclip" == 0 then
    return
  end

  local native = vim.fn.has "mac" == 1 or vim.env.WAYLAND_DISPLAY ~= nil or vim.env.DISPLAY ~= nil

  -- dclip drops one trailing newline; the extra one keeps linewise yanks linewise
  local function dclip(lines)
    vim.system({ "dclip" }, { stdin = table.concat(lines, "\n") .. "\n" }):wait()
  end

  -- children of nvim have no tty, so OSC 52 has to come from nvim itself
  local osc52 = require "vim.ui.clipboard.osc52"
  local paste = { "dclip", "-o" }

  vim.g.clipboard = {
    name = "dclip",
    copy = native and { ["+"] = dclip, ["*"] = dclip } or { ["+"] = osc52.copy "+", ["*"] = osc52.copy "*" },
    paste = { ["+"] = paste, ["*"] = paste },
  }
end

return M
