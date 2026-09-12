local function get_neo_tree_state()
  local tabpage = vim.api.nvim_get_current_tabpage()
  local current_window = vim.api.nvim_get_current_win()

  for _, window in ipairs(vim.api.nvim_tabpage_list_wins(tabpage)) do
    local buffer = vim.api.nvim_win_get_buf(window)
    if vim.bo[buffer].filetype == "neo-tree" then
      local source = vim.b[buffer].neo_tree_source or "filesystem"
      local position = vim.b[buffer].neo_tree_position
      local restart_state = {
        focused = window == current_window,
        position = position,
        source = source,
      }

      if source == "filesystem" then
        local ok, manager = pcall(require, "neo-tree.sources.manager")
        if ok then
          local state = manager.get_state(source, tabpage)
          local node = state.tree and state.tree:get_node()
          restart_state.node = node and node:get_id() or nil
          restart_state.root = state.path
        end
      end

      return restart_state
    end
  end
end

return get_neo_tree_state
