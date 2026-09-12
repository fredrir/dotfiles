local tooling = require "languages.tooling"

local M = {}

local function shell_root(bufnr, on_dir)
  local name = vim.api.nvim_buf_get_name(bufnr)
  name = vim.uv.fs_realpath(name) or name
  on_dir(vim.fs.root(name, { ".shuck.toml", "shuck.toml", ".git" }) or vim.fs.dirname(name))
end

---@return table<string, vim.lsp.Config>
function M.configs()
  ---@type table<string, vim.lsp.Config>
  local servers = {
    shuck = {
      cmd = { "shuck", "server" },
      filetypes = { "bash", "sh", "zsh" },
      init_options = { showSyntaxErrors = true },
      root_dir = shell_root,
      on_init = function(client)
        client.server_capabilities.completionProvider = nil
      end,
    },
    bashls = {
      cmd = { "bash-language-server", "start" },
      filetypes = { "bash", "sh", "zsh" },
      root_dir = shell_root,
      settings = {
        bashIde = {
          shellcheckPath = "",
          shfmt = { path = "" },
          enableSourceErrorDiagnostics = false,
        },
      },
      before_init = function(_, config)
        local root = config.root_dir
        local project = root and root ~= vim.uv.os_homedir() and vim.uv.fs_stat(root .. "/.git")
        local pattern = "{*.sh,*.inc,*.bash,*.zsh,*.command,.bash*,.zsh*,.zprofile,.zlogin,.zlogout,.profile}"
        config.settings.bashIde.globPattern = vim.env.GLOB_PATTERN or ((project and "**/" or "") .. pattern)
        config.settings.bashIde.includeAllWorkspaceSymbols = project ~= nil and project ~= false
      end,
      -- Bash parsing is useful for completion, but must not diagnose Zsh or replace Shuck's other features.
      handlers = { ["textDocument/publishDiagnostics"] = function() end },
      on_init = function(client)
        local caps = client.server_capabilities
        client.server_capabilities = {
          completionProvider = caps.completionProvider,
          textDocumentSync = caps.textDocumentSync,
          positionEncoding = caps.positionEncoding,
        }
      end,
    },
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
    biome = {},
    taplo = {
      cmd = function(dispatchers, config)
        local cmd = { "taplo", "lsp" }
        local root = (config or {}).root_dir
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

  if vim.fn.executable "go" == 1 then
    servers.gopls = {}
  end

  return servers
end

---@param servers table<string, vim.lsp.Config>
---@return string[]
function M.mason_tools(servers)
  local machine_tools = { biome = true, taplo = true, shuck = true }
  local ensure_installed = vim.tbl_filter(function(name)
    return not machine_tools[name]
  end, vim.tbl_keys(servers))

  if vim.fn.executable "go" == 1 then
    table.insert(ensure_installed, "goimports")
  end

  return ensure_installed
end

return M
