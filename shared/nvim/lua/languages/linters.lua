local tooling = require "languages.tooling"

local M = {}

local served_by = { biomejs = "biome" }

local inject = {
  -- yamllint looks for `.yamllint*` in the working directory and then
  yamllint = function(linter, bufnr)
    local names = { ".yamllint", ".yamllint.yaml", ".yamllint.yml" }
    local config = tooling.resolve(bufnr, names, ".yamllint.yaml")
    if config then
      table.insert(linter.args, 1, config)
      table.insert(linter.args, 1, "-c")
    end
  end,
  biomejs = function(linter, bufnr)
    local config = tooling.fallback(bufnr, { "biome.json", "biome.jsonc" }, "biome.global.json")
    if config then
      table.insert(linter.args, "--config-path=" .. config)
    end
  end,
}

M.by_filetype = {
  python = { "ruff" },
  yaml = { "yamllint" },
  sql = { "sqlfluff" },
  javascript = { "biomejs" },
  typescript = { "biomejs" },
  javascriptreact = { "biomejs" },
  typescriptreact = { "biomejs" },
  css = { "biomejs" },
  json = { "biomejs" },
  jsonc = { "biomejs" },
}

---@param linter table
---@param bufnr integer
---@return table
function M.wrap(linter, bufnr)
  local injector = inject[linter.name]
  if injector then
    injector(linter, bufnr)
  end
  return linter
end

---@param lint table
---@param linter table
---@param bufnr integer
---@return boolean
function M.not_already_served(lint, linter, bufnr)
  local client = served_by[linter.name]
  if client == nil or #vim.lsp.get_clients { bufnr = bufnr, name = client } == 0 then
    return true
  end
  vim.diagnostic.reset(lint.get_namespace(linter.name), bufnr)
  return false
end

return M
