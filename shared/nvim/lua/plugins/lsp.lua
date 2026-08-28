if vim.g.minimal then
  return {}
end -- skipped in minimal (server) mode
local tooling = require "tooling"
return {
  "neovim/nvim-lspconfig",
  dependencies = {
    -- `append`, not mason's default `prepend`. The formatters and linters this
    -- repo uses are machine tools (config/requirements.dotfile), and mason's
    -- bin directory in front of PATH meant conform silently ran mason's copy
    -- instead -- ruff 0.16.1 in the editor against 0.16.4 from `dotfile
    -- format` on this host. Appending leaves mason to fill gaps only.
    { "mason-org/mason.nvim", opts = { PATH = "append" } },
    -- Never `setup{}`: enabling servers is done by hand below. It is a
    -- dependency because mason-tool-installer reads its lspconfig-to-mason
    -- name table to turn `ts_ls` into `typescript-language-server`.
    "mason-org/mason-lspconfig.nvim",
    "WhoIsSethDaniel/mason-tool-installer.nvim",
    { "j-hui/fidget.nvim", opts = {} },
  },
  config = function()
    vim.api.nvim_create_autocmd("LspAttach", {
      group = vim.api.nvim_create_augroup("lsp-attach", { clear = true }),
      callback = function(event)
        local map = function(keys, func, desc, mode)
          mode = mode or "n"
          vim.keymap.set(mode, keys, func, { buffer = event.buf, desc = "LSP: " .. desc })
        end

        map("grn", vim.lsp.buf.rename, "[R]e[n]ame")
        map("<leader>rn", vim.lsp.buf.rename, "[R]e[n]ame")
        map("gra", vim.lsp.buf.code_action, "[G]oto Code [A]ction", { "n", "x" })
        map("grD", vim.lsp.buf.declaration, "[G]oto [D]eclaration")

        local client = vim.lsp.get_client_by_id(event.data.client_id)
        if client and client:supports_method("textDocument/documentHighlight", event.buf) then
          local highlight_augroup = vim.api.nvim_create_augroup("lsp-highlight", { clear = false })
          vim.api.nvim_create_autocmd({ "CursorHold", "CursorHoldI" }, {
            buffer = event.buf,
            group = highlight_augroup,
            callback = vim.lsp.buf.document_highlight,
          })
          vim.api.nvim_create_autocmd({ "CursorMoved", "CursorMovedI" }, {
            buffer = event.buf,
            group = highlight_augroup,
            callback = vim.lsp.buf.clear_references,
          })
          vim.api.nvim_create_autocmd("LspDetach", {
            group = vim.api.nvim_create_augroup("lsp-detach", { clear = true }),
            callback = function(event2)
              vim.lsp.buf.clear_references()
              vim.api.nvim_clear_autocmds { group = "lsp-highlight", buffer = event2.buf }
            end,
          })
        end

        if client and client:supports_method("textDocument/inlayHint", event.buf) then
          map("<leader>th", function()
            vim.lsp.inlay_hint.enable(not vim.lsp.inlay_hint.is_enabled { bufnr = event.buf })
          end, "[T]oggle Inlay [H]ints")
        end
      end,
    })

    ---@type table<string, vim.lsp.Config>
    local servers = {
      rust_analyzer = {},
      ts_ls = {
        settings = {
          typescript = { updateImportsOnFileMove = { enabled = "always" } },
          javascript = { updateImportsOnFileMove = { enabled = "always" } },
        },
      },
      cssls = {},
      html = {},
      tailwindcss = {},
      emmet_ls = {
        filetypes = { "html", "css", "javascriptreact", "typescriptreact" },
      },
      pyright = {},
      -- The same biome that conform formats with and that `dotfile format`
      -- runs, so its lint findings show up as you type instead of at commit
      -- time. lspconfig only attaches it inside a project that carries its own
      -- `biome.json`, which is what keeps that project's rules authoritative;
      -- everywhere else nvim-lint runs `biome lint` against the shipped config
      -- (see lint.lua, which skips itself wherever this server is attached).
      biome = {},
      -- taplo's diagnostics are the `taplo lint` half of `dotfile format
      -- --check`. It has to be told where the config is: like the CLI it only
      -- searches upward from the workspace for `.taplo.toml` and never reads
      -- `~/.config/taplo/taplo.toml`.
      taplo = {
        cmd = function(dispatchers, config)
          local cmd = { "taplo", "lsp" }
          local root = (config or {}).root_dir
          -- `.taplo.toml` is one of this server's root markers, so a workspace
          -- that has one has it exactly here.
          local own = root
            and (
              vim.uv.fs_stat(vim.fs.joinpath(root, ".taplo.toml"))
              or vim.uv.fs_stat(vim.fs.joinpath(root, "taplo.toml"))
            )
          local shared = not own and tooling.config ".taplo.toml" or nil
          if shared then
            vim.list_extend(cmd, { "--config", shared })
          end
          table.insert(cmd, "stdio")
          return vim.lsp.rpc.start(cmd, dispatchers)
        end,
      },
      -- jsonls and biome both want `json` and `jsonc`. They are split by what
      -- only one of them can do: jsonls validates against JSON Schema and
      -- completes keys from it, which biome has no notion of, so it keeps the
      -- buffer; biome formats and lints it, through conform in the editor and
      -- through `dotfile format` on the command line. jsonls' own formatter is
      -- therefore switched off -- left on, two servers advertise formatting
      -- for the same buffer and which one wins is down to attach order.
      jsonls = {
        init_options = { provideFormatter = false },
        settings = {
          json = {
            validate = { enable = true },
          },
        },
        filetypes = { "json", "jsonc" },
        capabilities = {
          textDocument = {
            diagnostic = vim.NIL,
          },
        },
      },
      lua_ls = {
        on_init = function(client)
          if client.workspace_folders then
            local path = client.workspace_folders[1].name
            if
              path ~= vim.fn.stdpath "config"
              and (vim.uv.fs_stat(path .. "/.luarc.json") or vim.uv.fs_stat(path .. "/.luarc.jsonc"))
            then
              return
            end
          end

          client.config.settings.Lua = vim.tbl_deep_extend("force", client.config.settings.Lua, {
            runtime = {
              version = "LuaJIT",
              path = { "lua/?.lua", "lua/?/init.lua" },
            },
            workspace = {
              checkThirdParty = false,
              library = vim.tbl_extend("force", vim.api.nvim_get_runtime_file("", true), {
                "${3rd}/luv/library",
                "${3rd}/busted/library",
              }),
            },
          })
        end,
        settings = { Lua = {} },
      },
    }

    local has_go = vim.fn.executable "go" == 1
    if has_go then
      servers.gopls = {}
    end

    -- mason installs language servers and nothing else. Every formatter and
    -- linter is a machine tool that config/requirements.dotfile tracks and brew
    -- or uv or bun installs, and it has to be that one copy: `dotfile format`
    -- can only reach the machine's, so a second one under mason is a second
    -- version formatting the same file. That drops stylua and ruff from this
    -- list, and eslint_d and prettierd with them -- the repo lints and formats
    -- web files with biome, and neither was even installed.
    --
    -- `biome` and `taplo` are the two servers that are also machine tools, so
    -- they are enabled above but left off here.
    local machine_tools = { biome = true, taplo = true }
    local ensure_installed = vim.tbl_filter(function(name)
      return not machine_tools[name]
    end, vim.tbl_keys(servers))
    -- The exception: nothing on this machine provides goimports, so
    -- `dotfile format` reports it missing and runs gofmt alone.
    if has_go then
      table.insert(ensure_installed, "goimports")
    end

    require("mason-tool-installer").setup { ensure_installed = ensure_installed }

    for name, server in pairs(servers) do
      vim.lsp.config(name, server)
      vim.lsp.enable(name)
    end
  end,
}
