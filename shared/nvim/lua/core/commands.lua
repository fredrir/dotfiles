local M = {}

vim.api.nvim_create_user_command("W", "w", {})
vim.api.nvim_create_user_command("Q", "q", {})
vim.api.nvim_create_user_command("WQ", "wq", {})
vim.api.nvim_create_user_command("Wq", "wq", {})

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

local function is_file_window(window)
  if not window or not vim.api.nvim_win_is_valid(window) then
    return false
  end

  local buffer = vim.api.nvim_win_get_buf(window)
  return vim.bo[buffer].buftype == "" and vim.api.nvim_buf_get_name(buffer) ~= ""
end

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

local function restore_editor(state)
  if type(state) ~= "table" or type(state.path) ~= "string" or state.path == "" then
    return
  end

  local current_window = vim.api.nvim_get_current_win()
  local editor_window

  for _, window in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    local buffer = vim.api.nvim_win_get_buf(window)
    local config = vim.api.nvim_win_get_config(window)
    if vim.bo[buffer].filetype ~= "neo-tree" and config.relative == "" then
      editor_window = window
      if window == current_window then
        break
      end
    end
  end

  if editor_window then
    vim.api.nvim_set_current_win(editor_window)
  end

  vim.cmd.edit(vim.fn.fnameescape(state.path))
  local window = vim.api.nvim_get_current_win()

  if type(state.view) == "table" then
    vim.api.nvim_win_call(window, function()
      vim.fn.winrestview(state.view)
    end)
  end

  return window
end

local function restore_neo_tree(state, editor_window)
  if type(state) ~= "table" then
    return
  end

  local args = {
    action = state.focused and "focus" or "show",
    position = state.position,
    source = state.source,
  }

  if state.source == "filesystem" then
    args.dir = state.root
    if state.node and (vim.uv or vim.loop).fs_stat(state.node) then
      args.reveal_file = state.node
    end
  end

  local loaded, neo_tree = pcall(require, "neo-tree.command")
  if not loaded then
    vim.notify("Could not restore Neo-tree after restart: " .. neo_tree, vim.log.levels.ERROR)
    return
  end

  local ok, err = pcall(neo_tree.execute, args)
  if not ok then
    vim.notify("Could not restore Neo-tree after restart: " .. err, vim.log.levels.ERROR)
    return
  end

  if not state.focused and editor_window and vim.api.nvim_win_is_valid(editor_window) then
    vim.api.nvim_set_current_win(editor_window)
  end
end

---@param payload string
function M.restore_restart(payload)
  local decoded, state = pcall(function()
    return vim.json.decode(vim.base64.decode(payload))
  end)
  if not decoded or type(state) ~= "table" then
    vim.notify("Could not restore Neo-tree after restart", vim.log.levels.ERROR)
    return
  end

  local restored = false
  local events
  local event_handler

  local function restore()
    if restored then
      return
    end
    restored = true

    if events and event_handler then
      events.unsubscribe(event_handler)
    end

    local editor_window = restore_editor(state.editor)
    restore_neo_tree(state.neo_tree, editor_window)

    if
      editor_window
      and vim.api.nvim_win_is_valid(editor_window)
      and type(state.editor) == "table"
      and type(state.editor.view) == "table"
    then
      vim.api.nvim_win_call(editor_window, function()
        vim.fn.winrestview(state.editor.view)
      end)
    end
  end

  local startup_path = vim.fn.argv(0)
  local startup_stat = type(startup_path) == "string" and (vim.uv or vim.loop).fs_stat(startup_path)
  if not startup_stat or startup_stat.type ~= "directory" then
    vim.schedule(restore)
    return
  end

  -- When Nvim was started with a directory, Neo-tree replaces that directory
  -- buffer on a debounced callback. Restore after its first filesystem render
  -- so the hijack cannot replace the editor buffer again.
  local loaded
  loaded, events = pcall(require, "neo-tree.events")
  if not loaded then
    vim.schedule(restore)
    return
  end

  local manager_loaded, manager = pcall(require, "neo-tree.sources.manager")
  local filesystem_state = manager_loaded and manager.get_state "filesystem"
  if filesystem_state and filesystem_state.winid and vim.api.nvim_win_is_valid(filesystem_state.winid) then
    vim.schedule(restore)
    return
  end

  event_handler = {
    event = events.NEO_TREE_WINDOW_AFTER_OPEN,
    id = "core_restart_restore",
    handler = function(args)
      if args.source == "filesystem" then
        -- The hijack queues its directory-buffer cleanup after opening the
        -- window. Two schedules put restoration behind that cleanup.
        vim.schedule(function()
          vim.schedule(restore)
        end)
      end
    end,
  }
  events.subscribe(event_handler)
end

function M.restart()
  local modified = get_modified_buffers()
  if #modified > 0 then
    vim.notify(
      ("Restart cancelled: save or discard modified buffers first:\n%s"):format(table.concat(modified, "\n")),
      vim.log.levels.WARN
    )
    return
  end

  local state = {
    editor = get_editor_state(),
    neo_tree = get_neo_tree_state(),
  }
  local payload = vim.base64.encode(vim.json.encode(state))

  -- Native session restoration serializes plugin and placeholder buffers.
  -- Skip it and restore only the real editor and Neo-tree state captured above.
  local command = ('restart! lua require("core.commands").restore_restart(%q)'):format(payload)
  local ok, err = pcall(vim.cmd, command)

  if not ok then
    error(err, 0)
  end
end

return M
