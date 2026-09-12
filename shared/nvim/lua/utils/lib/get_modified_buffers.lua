local function get_modified_buffers()
  local modified = {}

  for _, buffer in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(buffer) and vim.bo[buffer].modified then
      local name = vim.api.nvim_buf_get_name(buffer)
      modified[#modified + 1] = name == "" and "[No Name]" or vim.fn.fnamemodify(name, ":~:.")
    end
  end

  table.sort(modified)
  return modified
end

return get_modified_buffers
