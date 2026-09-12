local is_file_window = require "utils.lib.is_file_window"

local function get_editor_state()
  local tabpage = vim.api.nvim_get_current_tabpage()
  local current_window = vim.api.nvim_get_current_win()
  local editor_window

  if is_file_window(current_window) then
    editor_window = current_window
  else
    local alternate_window = vim.fn.win_getid(vim.fn.winnr "#")
    if is_file_window(alternate_window) and vim.api.nvim_win_get_tabpage(alternate_window) == tabpage then
      editor_window = alternate_window
    else
      for _, window in ipairs(vim.api.nvim_tabpage_list_wins(tabpage)) do
        if is_file_window(window) then
          editor_window = window
          break
        end
      end
    end
  end

  if not editor_window then
    return
  end

  local buffer = vim.api.nvim_win_get_buf(editor_window)
  return {
    path = vim.api.nvim_buf_get_name(buffer),
    view = vim.api.nvim_win_call(editor_window, vim.fn.winsaveview),
  }
end

return get_editor_state
