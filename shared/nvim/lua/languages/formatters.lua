local tooling = require "languages.tooling"

---@module 'conform'
---@type conform.setupOpts
return {
  notify_on_error = false,
  format_on_save = function(bufnr)
    local disable_filetypes = { c = true, cpp = true }
    if disable_filetypes[vim.bo[bufnr].filetype] then
      return nil
    end
    return { timeout_ms = 500, lsp_format = "fallback" }
  end,
  formatters_by_ft = {
    lua = { "stylua" },
    python = { "ruff_format" },
    go = { "goimports", "gofmt" },
    javascript = { "biome" },
    typescript = { "biome" },
    javascriptreact = { "biome" },
    typescriptreact = { "biome" },
    css = { "biome" },
    html = { "biome" },
    json = { "biome" },
    jsonc = { "biome" },
    yaml = { "yamlfmt" },
    sql = { "sqlfluff" },
    toml = { "taplo" },
    sh = { "shfmt" },
    bash = { "shfmt" },
    conf = { "dotfmt" },
    dotfile = { "dotfmt" },
  },
  formatters = {
    dotfmt = {
      command = "dotfmt",
      args = { "--stdin", "$FILENAME" },
      stdin = true,
    },
    stylua = {
      args = function(_, ctx)
        local args = { "--search-parent-directories", "--respect-ignores" }
        local config = tooling.fallback(ctx.buf, { ".stylua.toml", "stylua.toml" }, "stylua.toml")
        if config then
          vim.list_extend(args, { "--config-path", config })
        end
        vim.list_extend(args, { "--stdin-filepath", "$FILENAME", "-" })
        return args
      end,
    },
    taplo = {
      args = function(_, ctx)
        local args = { "format" }
        local config = tooling.fallback(ctx.buf, { ".taplo.toml", "taplo.toml" }, ".taplo.toml")
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
        local config = tooling.fallback(ctx.buf, { "biome.json", "biome.jsonc" }, "biome.global.json")
        if config then
          table.insert(args, "--config-path=" .. config)
        end
        return args
      end,
    },
    sqlfluff = {
      args = { "format", "-" },
      require_cwd = false,
    },
  },
}
