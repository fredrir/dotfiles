if vim.g.minimal then
  return {}
end -- skipped in minimal (server) mode
local tooling = require "tooling"

-- Linters an attached language server already publishes the same findings for.
-- Running both puts every one of them in the buffer twice.
local served_by = { biomejs = "biome" }

-- Config the linter cannot find for itself. `dotfile format --check` reaches
-- these through `shared/tools/`; without the same files here the editor reports
-- against a different rule set than the commit hook does.
local inject = {
  -- yamllint looks for `.yamllint*` in the working directory and then
  -- `~/.config/yamllint/config`. `~/.yamllint.yaml` is neither, and an editor's
  -- working directory has nothing to do with the buffer anyway, so left alone
  -- it lints at yamllint's defaults: line length 80 and an error, against this
  -- repo's 100 and a warning.
  yamllint = function(linter, bufnr)
    local names = { ".yamllint", ".yamllint.yaml", ".yamllint.yml" }
    local config = tooling.resolve(bufnr, names, ".yamllint.yaml")
    if config then
      table.insert(linter.args, 1, config)
      table.insert(linter.args, 1, "-c")
    end
  end,
  -- biome resolves `biome.json` upward from its own working directory, so
  -- outside a biome project it lints with its built-in defaults rather than the
  -- shipped rules. The file name nvim-lint appends stays last.
  biomejs = function(linter, bufnr)
    local config = tooling.fallback(bufnr, { "biome.json", "biome.jsonc" }, "biome.global.json")
    if config then
      table.insert(linter.args, "--config-path=" .. config)
    end
  end,
}

return {
  "mfussenegger/nvim-lint",
  event = { "BufReadPre", "BufNewFile" },
  config = function()
    local lint = require "lint"

    -- The set `dotfile format --check` runs -- ruff check, biome lint, taplo
    -- lint, yamllint, sqlfluff lint -- minus the one that arrives over LSP
    -- instead: `toml` is the taplo server, configured in lsp.lua. ruff and
    -- sqlfluff need nothing here, they read `~/.config/ruff/ruff.toml` and
    -- `~/.sqlfluff` on their own from wherever they are run.
    lint.linters_by_ft = {
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

    local function wrap(linter)
      local injector = inject[linter.name]
      if injector then
        injector(linter, vim.api.nvim_get_current_buf())
      end
      return linter
    end

    -- A server attaches well after the first `BufEnter` lint has already run,
    -- so filtering is not enough on its own: whatever that first pass reported
    -- would sit in the buffer forever, never refreshed and now duplicated by
    -- the server. Clear the linter's namespace as we step aside.
    local function not_already_served(linter)
      local client = served_by[linter.name]
      if client == nil or #vim.lsp.get_clients { bufnr = 0, name = client } == 0 then
        return true
      end
      vim.diagnostic.reset(lint.get_namespace(linter.name), vim.api.nvim_get_current_buf())
      return false
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
