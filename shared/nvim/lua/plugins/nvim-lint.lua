local tools = require "utils.tools"

local served_by = { biomejs = "biome" }

---@type LazySpec
return {
  {
    "mfussenegger/nvim-lint",
    event = { "BufReadPre", "BufNewFile" },
    config = function()
      local lint = require "lint"

      lint.linters_by_ft = {
        css = { "biomejs" },
        javascript = { "biomejs" },
        javascriptreact = { "biomejs" },
        jsonc = { "biomejs" },
        python = { "ruff" },
        sql = { "sqlfluff" },
        yaml = { "yamllint" },
      }

      local inject = {
        yamllint = function(linter, bufnr)
          local config = tools.project(bufnr, { ".yamllint", ".yamllint.yaml", ".yamllint.yml" })
            or tools.shared ".yamllint.yaml"
          if config then
            linter.args = linter.args or {}
            table.insert(linter.args, 1, config)
            table.insert(linter.args, 1, "-c")
          end
        end,
        biomejs = function(linter, bufnr)
          local config = not tools.project(bufnr, { "biome.json", "biome.jsonc" }) and tools.shared "biome.global.json"
            or nil
          if config then
            linter.args = linter.args or {}
            table.insert(linter.args, "--config-path=" .. config)
          end
        end,
      }

      vim.api.nvim_create_autocmd({ "BufEnter", "BufWritePost", "InsertLeave" }, {
        group = vim.api.nvim_create_augroup("lint", { clear = true }),
        callback = function(event)
          local bufnr = event.buf
          if not vim.bo[bufnr].modifiable or vim.bo[bufnr].buftype ~= "" then
            return
          end
          vim.api.nvim_buf_call(bufnr, function()
            lint.try_lint(nil, {
              wrap_linter = function(linter)
                local injector = inject[linter.name]
                if injector then
                  injector(linter, bufnr)
                end
                return linter
              end,
              filter = function(linter)
                local client = served_by[linter.name]
                if client == nil or #vim.lsp.get_clients { bufnr = bufnr, name = client } == 0 then
                  return true
                end
                vim.diagnostic.reset(lint.get_namespace(linter.name), bufnr)
                return false
              end,
            })
          end)
        end,
      })
    end,
  },
}
