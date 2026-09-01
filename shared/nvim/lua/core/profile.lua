local minimal = vim.env.NVIM_MINIMAL

local M = {
  minimal = minimal ~= nil and minimal ~= "" and minimal ~= "0",
}

vim.g.minimal = M.minimal

return M
