local M = {}

function M.file_context()
  local bufnr = vim.api.nvim_get_current_buf()

  -- Filename
  local path = vim.api.nvim_buf_get_name(bufnr)
  local filename = path ~= "" and vim.fn.fnamemodify(path, ":t") or "[No Name]"

  -- Use extension for Markdown code fence.
  -- Fall back to Neovim filetype if there's no extension.
  local extension = vim.fn.fnamemodify(filename, ":e")
  local language = extension ~= "" and extension or vim.bo[bufnr].filetype

  -- Current buffer contents (includes unsaved changes)
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local content = table.concat(lines, "\n")

  -- Diagnostics
  local diagnostics = vim.diagnostic.get(bufnr)

  local severity_names = {
    [vim.diagnostic.severity.ERROR] = "ERROR",
    [vim.diagnostic.severity.WARN] = "WARN",
    [vim.diagnostic.severity.INFO] = "INFO",
    [vim.diagnostic.severity.HINT] = "HINT",
  }

  -- Sort diagnostics by line, then column
  table.sort(diagnostics, function(a, b)
    if a.lnum == b.lnum then
      return (a.col or 0) < (b.col or 0)
    end

    return a.lnum < b.lnum
  end)

  local diagnostic_lines = {}

  for _, diagnostic in ipairs(diagnostics) do
    local severity = severity_names[diagnostic.severity] or "UNKNOWN"
    local line = diagnostic.lnum + 1
    local column = (diagnostic.col or 0) + 1

    -- Make multiline diagnostic messages easier to paste/read
    local message = diagnostic.message:gsub("\n", " ")

    local source = diagnostic.source and string.format(" [%s]", diagnostic.source) or ""

    local code = diagnostic.code and string.format(" (%s)", tostring(diagnostic.code)) or ""

    table.insert(diagnostic_lines, string.format("%s L%d:C%d%s%s: %s", severity, line, column, source, code, message))
  end

  if #diagnostic_lines == 0 then
    diagnostic_lines = { "No diagnostics." }
  end

  local output = table.concat({
    "---",
    filename,
    "```" .. language,
    content,
    "```",
    "",
    "Diagnostics",
    "```",
    table.concat(diagnostic_lines, "\n"),
    "```",
    "---",
  }, "\n")

  -- Copy to system clipboard
  vim.fn.setreg("+", output)

  vim.notify "Copied file + diagnostics to clipboard"
end

return M
