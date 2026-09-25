local tools = require "utils.tools"

local machine_tools = { "biome", "shucked", "taplo" }

local servers = {
  "cssls",
  "emmet_ls",
  "html",
  "jsonls",
  "lua_ls",
  "pyright",
  "rust_analyzer",
  "tailwindcss",
  "ts_ls",
}
if vim.fn.executable "go" == 1 then
  servers[#servers + 1] = "gopls"
end

local function configure()
  vim.lsp.config("shucked", {
    cmd = { "shucked", "server" },
    filetypes = { "bash", "sh", "zsh" },
    root_markers = { ".shucked.toml", "shucked.toml", ".git" },
    init_options = { showSyntaxErrors = true },
    on_init = function(client)
      client.server_capabilities.completionProvider = nil
    end,
  })

  vim.lsp.config("lua_ls", {
    settings = { Lua = {} },
    on_init = function(client)
      if client.workspace_folders then
        local path = client.workspace_folders[1].name
        if path ~= vim.fn.stdpath "config" then
          local own = vim.uv.fs_stat(path .. "/.luarc.json") or vim.uv.fs_stat(path .. "/.luarc.jsonc")
          if own then
            return
          end
        end
      end

      local settings = client.settings.Lua
      client.settings.Lua = vim.tbl_deep_extend("force", type(settings) == "table" and settings or {}, {
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
  })

  vim.lsp.config("jsonls", {
    filetypes = { "json", "jsonc" },
    init_options = { provideFormatter = false },
    settings = { json = { validate = { enable = true } } },
    on_init = function(client)
      client.server_capabilities.diagnosticProvider = nil
    end,
  })

  vim.lsp.config("ts_ls", {
    settings = {
      typescript = { updateImportsOnFileMove = { enabled = "always" } },
      javascript = { updateImportsOnFileMove = { enabled = "always" } },
    },
  })

  vim.lsp.config("emmet_ls", {
    filetypes = { "html", "css", "javascriptreact", "typescriptreact" },
  })

  vim.lsp.config("taplo", {
    cmd = function(dispatchers, config)
      local cmd = { "taplo", "lsp" }
      local root = (config or {}).root_dir
      local own = root
        and (
          vim.uv.fs_stat(vim.fs.joinpath(root, ".taplo.toml"))
          or vim.uv.fs_stat(vim.fs.joinpath(root, "taplo.toml"))
        )
      if not own then
        local shared = tools.shared ".taplo.toml"
        if shared then
          vim.list_extend(cmd, { "--config", shared })
        end
      end
      cmd[#cmd + 1] = "stdio"
      return vim.lsp.rpc.start(cmd, dispatchers)
    end,
  })
end

---@type LazySpec
return {
  {
    "neovim/nvim-lspconfig",
    event = { "BufReadPre", "BufNewFile" },
    dependencies = {
      { "mason-org/mason.nvim", opts = { PATH = "append" } },
      "mason-org/mason-lspconfig.nvim",
      { "j-hui/fidget.nvim", opts = {} },
    },
    config = function()
      configure()
      require("mason-lspconfig").setup { ensure_installed = servers, automatic_enable = false }

      local enabled = {}
      vim.list_extend(enabled, servers)
      vim.list_extend(enabled, machine_tools)
      vim.lsp.enable(enabled)
    end,
  },
}
