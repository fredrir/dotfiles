local profile = require "core.profile"

if profile.minimal then
  return {}
end

local linters = require "languages.linters"

return {
  "mfussenegger/nvim-lint",
  event = { "BufReadPre", "BufNewFile" },
  config = function()
    local lint = require "lint"
    lint.linters_by_ft = linters.by_filetype

    local function wrap(linter)
      return linters.wrap(linter, vim.api.nvim_get_current_buf())
    end

    local function not_already_served(linter)
      return linters.not_already_served(lint, linter, vim.api.nvim_get_current_buf())
    end

    local lint_augroup = vim.api.nvim_create_augroup("lint", { clear = true })
    vim.api.nvim_create_autocmd({ "BufEnter", "BufWritePost", "InsertLeave" }, {
      group = lint_augroup,
      callback = function()
        if vim.bo.modifiable then
          lint.try_lint(nil, { wrap_linter = wrap, filter = not_already_served })
        end
      end,
    })
  end,
}
