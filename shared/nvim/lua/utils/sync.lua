local restart = require "utils.restart"

local M = {}

function M.neovim()
  local ok, lazy = pcall(require, "lazy")

  if not ok then
    vim.notify("Could not load lazy.nvim: " .. tostring(lazy), vim.log.levels.ERROR)
    return
  end

  local synced, err = pcall(function()
    lazy.sync {
      wait = true,
      show = false,
    }
  end)

  if not synced then
    vim.notify("Lazy sync failed: " .. tostring(err), vim.log.levels.ERROR)
    return
  end

  restart.neovim()
end

return M
