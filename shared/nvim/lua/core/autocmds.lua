vim.api.nvim_create_autocmd({ "FocusGained", "BufEnter", "CursorHold" }, {
  group = vim.api.nvim_create_augroup("checktime", { clear = true }),
  command = "checktime",
})

vim.api.nvim_create_autocmd("TextYankPost", {
  desc = "Highlight yanks",
  group = vim.api.nvim_create_augroup("highlight-yank", { clear = true }),
  callback = function()
    vim.hl.on_yank()
  end,
})
