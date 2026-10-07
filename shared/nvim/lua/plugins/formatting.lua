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
        json = { "dotfmt_json" },
        jsonc = { "dotfmt_jsonc" },
        lua = { "dotfmt_lua" },
        markdown = { "dotfmt_markdown" },
        python = { "ruff_format" },
        sql = { "sqlfluff" },
        toml = { "taplo" },
        typescript = { "biome" },
        typescriptreact = { "biome" },
        yaml = { "yamlfmt" },
        conf = { "dotfmt_conf" },
        dotfile = { "dotfmt_conf" },
      },
      formatters = {
        dotfmt_conf = {
          command = "dotfmt",
          args = { "-l", "conf", "-eq", "--stdin", "$FILENAME" },
          stdin = true,
        },
        dotfmt_json = {
          command = "dotfmt",
          args = { "-l", "json", "-eq", "--stdin", "$FILENAME" },
          stdin = true,
        },
        dotfmt_jsonc = {
          command = "dotfmt",
          args = { "-l", "json", "-q", "--dialect", "jsonc", "--stdin", "$FILENAME" },
          stdin = true,
        },
        dotfmt_lua = {
          command = "dotfmt",
          args = { "-l", "lua", "-eq", "--stdin", "$FILENAME" },
          stdin = true,
        },
        dotfmt_markdown = {
          command = "dotfmt",
          args = { "-l", "markdown", "-eq", "--stdin", "$FILENAME" },
          stdin = true,
        },
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