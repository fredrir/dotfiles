local lib = require "utils.lib"

local M = {}

function M.neovim()
  local modified = lib.get_modified_buffers()
  if #modified > 0 then
    vim.notify(
      ("Restart cancelled: save or discard modified buffers first:\n%s"):format(table.concat(modified, "\n")),
      vim.log.levels.WARN
    )
    return
  end

  local state = {
    editor = lib.get_editor_state(),
    neo_tree = lib.get_neo_tree_state(),
  }
  local payload = vim.base64.encode(vim.json.encode(state))

  -- Native session restoration serializes plugin and placeholder buffers.
  -- Skip it and restore only the real editor and Neo-tree state captured above.
  local command = ("restart! lua require('utils.restore').restart(%q)"):format(payload)
  local ok, err = pcall(vim.cmd, command)

  if not ok then
    error(err, 0)
  end
end

return M
