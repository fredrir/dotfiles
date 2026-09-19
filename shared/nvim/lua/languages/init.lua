local lib = require "lib"

---@class Language
---@field filetypes? string[]
---@field servers? string[]
---@field formatters? string[]
---@field linters? string[]
---@field parsers? string[]
---@field debuggers? string[]
---@field tools? string[] Additional Mason tools
---@field extensions? table<string, string>

---@type table<string, Language>
local catalog = {
  shell = { filetypes = { "bash", "sh", "zsh" }, servers = { "shucked", "bashls" }, parsers = { "bash", "zsh" } },
  lua = { filetypes = { "lua" }, servers = { "lua_ls" }, formatters = { "stylua" }, parsers = { "lua", "luadoc" } },
  python = {
    filetypes = { "python" },
    servers = { "pyright" },
    formatters = { "ruff_format" },
    linters = { "ruff" },
    parsers = { "python" },
  },
  go = {
    filetypes = { "go" },
    servers = { "gopls" },
    formatters = { "goimports", "gofmt" },
    parsers = { "go" },
    debuggers = { "delve" },
    tools = { "goimports" },
  },
  rust = { filetypes = { "rust" }, servers = { "rust_analyzer" } },
  javascript = {
    filetypes = { "javascript", "javascriptreact", "typescript", "typescriptreact" },
    servers = { "ts_ls", "biome", "tailwindcss", "emmet_ls" },
    formatters = { "biome" },
    linters = { "biomejs" },
    parsers = { "javascript", "typescript", "tsx" },
  },
  css = {
    filetypes = { "css" },
    servers = { "cssls", "biome", "tailwindcss", "emmet_ls" },
    formatters = { "biome" },
    linters = { "biomejs" },
    parsers = { "css" },
  },
  html = {
    filetypes = { "html" },
    servers = { "html", "tailwindcss", "emmet_ls" },
    formatters = { "biome" },
    parsers = { "html" },
  },
  yaml = { filetypes = { "yaml" }, formatters = { "yamlfmt" }, linters = { "yamllint" }, parsers = { "yaml" } },
  sql = { filetypes = { "sql" }, formatters = { "sqlfluff" }, linters = { "sqlfluff" } },
  toml = { filetypes = { "toml" }, servers = { "taplo" }, formatters = { "taplo" } },
  json = {
    filetypes = { "json" },
    formatters = { "jqfmt" },
  },
  jsonc = {
    filetypes = { "jsonc" },
    servers = { "jsonls", "biome" },
    formatters = { "biome" },
    linters = { "biomejs" },
    parsers = { "json" },
  },

  dotfile = {
    filetypes = { "conf", "dotfile" },
    formatters = { "dotfmt" },
    extensions = { config = "dotfile", dotfile = "dotfile" },
  },
  markdown = { filetypes = { "markdown" }, parsers = { "markdown", "markdown_inline" } },
  c = { filetypes = { "c" }, parsers = { "c" } },
  diff = { filetypes = { "diff" }, parsers = { "diff" } },
  query = { filetypes = { "query" }, parsers = { "query" } },
  vim = { filetypes = { "vim" }, parsers = { "vim", "vimdoc" } },
}

local M = { catalog = catalog }

---@param field "servers"|"formatters"|"linters"|"parsers"|"debuggers"|"tools"
---@return string[]
function M.list(field)
  local values = {}
  for _, language in pairs(catalog) do
    for _, name in ipairs(language[field] or {}) do
      values[#values + 1] = name
    end
  end
  local result = lib.unique(values)
  table.sort(result)
  return result
end

---@param field "formatters"|"linters"
---@return table<string, string[]>
function M.by_filetype(field)
  local result = {}
  for _, language in pairs(catalog) do
    for _, filetype in ipairs(language.filetypes or {}) do
      if language[field] then
        result[filetype] = { unpack(language[field]) }
      end
    end
  end
  return result
end

---@param language string
---@return table<string, boolean>
function M.filetypes(language)
  return lib.set(catalog[language] and catalog[language].filetypes or {})
end

---@return table<string, string>
function M.extensions()
  local extensions = {}
  for _, language in pairs(catalog) do
    for extension, filetype in pairs(language.extensions or {}) do
      extensions[extension] = filetype
    end
  end
  return extensions
end

return M
