local M = {}

M.filetypes = { sh = true, bash = true, zsh = true }

function M.show_completion(cmp)
  if not M.filetypes[vim.bo.filetype] then
    return
  end
  local before = vim.api.nvim_get_current_line():sub(1, vim.api.nvim_win_get_cursor(0)[2])
  if before:match "%S$" then
    return cmp.show { initial_selected_item_idx = 1 }
  end
end

function M.format_result(bufnr, err)
  if vim.api.nvim_buf_is_valid(bufnr) and err then
    vim.notify(type(err) == "string" and err or vim.inspect(err), vim.log.levels.ERROR)
  end
end

function M.format(opts)
  local bufnr = opts.bufnr or vim.api.nvim_get_current_buf()
  if M.filetypes[vim.bo[bufnr].filetype] then
    opts = vim.tbl_extend("force", opts, { bufnr = bufnr, lsp_format = "prefer", name = "shuck" })
    local changedtick = vim.api.nvim_buf_get_changedtick(bufnr)
    require("conform").format(opts, function(err)
      if err and vim.api.nvim_buf_is_valid(bufnr) and vim.api.nvim_buf_get_changedtick(bufnr) ~= changedtick then
        return
      end
      M.format_result(bufnr, err)
    end)
  else
    require("conform").format(opts)
  end
end

return M
