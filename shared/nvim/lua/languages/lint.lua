local tooling = require "utils.editor"

local M = {}

local served_by = { biomejs = "biome" }

local inject = {
  yamllint = function(linter, bufnr)
    local names = { ".yamllint", ".yamllint.yaml", ".yamllint.yml" }
    local config = tooling.resolve_config(bufnr, names, ".yamllint.yaml")
    if config then
      linter.args = linter.args or {}
      table.insert(linter.args, 1, config)
      table.insert(linter.args, 1, "-c")
    end
  end,
  biomejs = function(linter, bufnr)
    local config = tooling.fallback_config(bufnr, { "biome.json", "biome.jsonc" }, "biome.global.json")
    if config then
      linter.args = linter.args or {}
      table.insert(linter.args, "--config-path=" .. config)
    end
  end,
}

---@param linter lint.Linter
---@param bufnr integer
---@return lint.Linter
function M.wrap(linter, bufnr)
  local injector = inject[linter.name]
  if injector then
    injector(linter, bufnr)
  end
  return linter
end

---@param lint { get_namespace: fun(name: string): integer }
---@param linter lint.Linter
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

---@param event vim.api.keyset.create_autocmd.callback_args
function M.on_event(event)
  if not vim.bo[event.buf].modifiable or vim.bo[event.buf].buftype ~= "" then
    return
  end
  vim.api.nvim_buf_call(event.buf, function()
    local lint = require "lint"
    lint.try_lint(nil, {
      wrap_linter = function(linter)
        return M.wrap(linter, event.buf)
      end,
      filter = function(linter)
        return M.not_already_served(lint, linter, event.buf)
      end,
    })
  end)
end

---@param events string[]
function M.setup(events)
  require("lint").linters_by_ft = require("languages").by_filetype "linters"
  vim.api.nvim_create_autocmd(events, {
    group = vim.api.nvim_create_augroup("lint", { clear = true }),
    callback = M.on_event,
  })
end

return M
