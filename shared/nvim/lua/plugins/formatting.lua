local tools = require "utils.tools"

local function fallback(bufnr, markers, shared)
  return not tools.project(bufnr, markers) and tools.shared(shared) or nil
end

---@type LazySpec
return {
  {
    "stevearc/conform.nvim",
    event = "BufWritePre",
    cmd = "ConformInfo",
    opts = {
      notify_on_error = false,
      format_on_save = { timeout_ms = 500, lsp_format = "fallback" },
      formatters_by_ft = {
        css = { "biome" },
        html = { "biome" },
        javascript = { "biome" },
        javascriptreact = { "biome" },
        json = { "jqfmt" },
        jsonc = { "biome" },
        lua = { "luafmt" },
        markdown = { "mdfmt" },
        python = { "ruff_format" },
        sql = { "sqlfluff" },
        toml = { "taplo" },
        typescript = { "biome" },
        typescriptreact = { "biome" },
        yaml = { "yamlfmt" },
        conf = { "dotfmt" },
        dotfile = { "dotfmt" },
      },
      formatters = {
        dotfmt = { command = "dotfmt", args = { "--stdin", "$FILENAME" }, stdin = true },
        jqfmt = { command = "jqfmt", args = { "-eq" }, stdin = true },
        luafmt = { command = "luafmt", args = { "-eq", "--stdin", "$FILENAME" }, stdin = true },
        mdfmt = { command = "mdfmt", args = { "-eq", "--stdin", "$FILENAME" }, stdin = true },
        sqlfluff = { args = { "format", "-" }, require_cwd = false },
        taplo = {
          args = function(_, ctx)
            local args = { "format" }
            local config = fallback(ctx.buf, { ".taplo.toml", "taplo.toml" }, ".taplo.toml")
            if config then
              vim.list_extend(args, { "--config", config })
            end
            vim.list_extend(args, { "--stdin-filepath", "$FILENAME", "-" })
            return args
          end,
        },
        biome = {
          args = function(_, ctx)
            local args = { "format", "--stdin-file-path", "$FILENAME" }
            local config = fallback(ctx.buf, { "biome.json", "biome.jsonc" }, "biome.global.json")
            if config then
              args[#args + 1] = "--config-path=" .. config
            end
            return args
          end,
        },
      },
    },
  },
}