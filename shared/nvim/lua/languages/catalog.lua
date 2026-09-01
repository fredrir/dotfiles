local M = {}

local parsers = {
  minimal = {
    "bash",
    "c",
    "diff",
    "go",
    "json",
    "lua",
    "luadoc",
    "markdown",
    "markdown_inline",
    "python",
    "query",
    "vim",
    "vimdoc",
    "yaml",
  },
  full = {
    "bash",
    "c",
    "css",
    "diff",
    "go",
    "html",
    "javascript",
    "json",
    "lua",
    "luadoc",
    "markdown",
    "markdown_inline",
    "python",
    "query",
    "tsx",
    "typescript",
    "vim",
    "vimdoc",
    "yaml",
  },
}

---@param profile { minimal: boolean }
---@return string[]
function M.parsers_for(profile)
  return profile.minimal and parsers.minimal or parsers.full
end

return M
