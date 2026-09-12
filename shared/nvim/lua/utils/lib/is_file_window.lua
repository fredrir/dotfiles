local function is_file_window(window)
  if not window or not vim.api.nvim_win_is_valid(window) then
    return false
  end

  local buffer = vim.api.nvim_win_get_buf(window)
  return vim.bo[buffer].buftype == "" and vim.api.nvim_buf_get_name(buffer) ~= ""
end

return is_file_window
