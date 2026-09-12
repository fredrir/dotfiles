local M = {}

M.neotree = {
  ["'"] = function()
    vim.cmd "wincmd p"
  end,
}

return M
