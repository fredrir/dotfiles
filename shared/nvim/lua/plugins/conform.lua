if vim.g.minimal then
  return {}
end -- skipped in minimal (server) mode
local tooling = require "tooling"
return {
  "stevearc/conform.nvim",
  event = { "BufWritePre" },
  cmd = { "ConformInfo" },
  keys = {
    {
      "<leader>f",
      function()
        require("conform").format { async = true, lsp_format = "fallback" }
      end,
      mode = "",
      desc = "[F]ormat buffer",
    },
  },
  -- Nothing detects these otherwise: neovim has no `dotfile` filetype, and
  -- `.config` reads as plain text. `.conf` keeps its own filetype and is
  -- pointed at dotfmt below.
  init = function()
    vim.filetype.add { extension = { config = "dotfile", dotfile = "dotfile" } }
  end,
  ---@module 'conform'
  ---@type conform.setupOpts
  opts = {
    notify_on_error = false,
    format_on_save = function(bufnr)
      local disable_filetypes = { c = true, cpp = true }
      if disable_filetypes[vim.bo[bufnr].filetype] then
        return nil
      end
      return { timeout_ms = 500, lsp_format = "fallback" }
    end,
    -- One row per `dotfile format` provider, so a buffer written here and the
    -- same file run through the CLI come out identical. Rust is missing on
    -- purpose: `cargo fmt` has no per-file mode, and rust_analyzer formats with
    -- rustfmt and the project's own `rustfmt.toml`, which is the same thing.
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
      -- Every diagnostic dotfmt has goes to stderr, so stdout is the buffer
      -- and nothing else; on a parse error it writes none of it and exits
      -- non-zero, which notify_on_error = false above turns into a no-op.
      dotfmt = {
        command = "dotfmt",
        args = { "--stdin", "$FILENAME" },
        stdin = true,
      },
      -- Left alone this works only by luck: conform passes
      -- `--search-parent-directories`, which walks past the filesystem root to
      -- `~/.config/stylua/stylua.toml` -- a symlink `dotfile link` happens to
      -- have made. On a fresh clone it is not there and the editor silently
      -- formats Lua with tabs. Name the file instead, like taplo and biome.
      -- `--config-path` outranks every nearer `.stylua.toml`, so it may only be
      -- passed where there is none; that is what `fallback` decides.
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
      -- taplo searches upward from the file for `.taplo.toml` and stops there;
      -- it never reads `~/.config/taplo/taplo.toml`. This repo has no
      -- `.taplo.toml` of its own, so without the flag the editor formatted TOML
      -- at taplo's defaults while the CLI used this repo's settings.
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
      -- Same hole, and conform's shipped entry papers over it by passing the
      -- buffer's own `expandtab`/`shiftwidth` whenever it finds no `biome.json`
      -- -- so web files and JSON were formatted to whatever the buffer happened
      -- to be set to. Name the shipped config instead. `--config-path` takes a
      -- file, which matters here: the repo's copy is `biome.global.json` so
      -- that linking it into `$HOME` cannot shadow a project's own.
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
      -- `dotfile format` runs `sqlfluff format`, which applies the layout rules
      -- only; conform ships `sqlfluff fix`, which applies every fixable rule and
      -- will rewrite the query itself. `require_cwd` goes because conform skips
      -- the formatter outright without a `.sqlfluff` above the buffer, and
      -- sqlfluff reads `~/.sqlfluff` wherever it is run from.
      sqlfluff = {
        args = { "format", "-" },
        require_cwd = false,
      },
    },
  },
}
