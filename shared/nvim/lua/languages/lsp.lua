local tooling = require "utils.editor"
local languages = require "languages"

local M = {}

local function shell_root(bufnr, on_dir)
  local name = vim.api.nvim_buf_get_name(bufnr)
  name = vim.uv.fs_realpath(name) or name
  on_dir(vim.fs.root(name, { ".shucked.toml", "shucked.toml", ".git" }) or vim.fs.dirname(name))
end

---@return table<string, vim.lsp.Config>
function M.configs()
  ---@type table<string, vim.lsp.Config>
  local servers = {
    shucked = {
      cmd = { "shucked", "server" },
      filetypes = languages.catalog.shell and languages.catalog.shell.filetypes or {},
      init_options = { showSyntaxErrors = true },
      root_dir = shell_root,
      on_init = function(client)
        client.server_capabilities.completionProvider = nil
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
        local shared = not own and tooling.shared_config ".taplo.toml" or nil
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
      -- Neovim merges default capabilities before this hook. Keep JSON push diagnostics.
      before_init = function(params)
        local text_document = params.capabilities.textDocument
        if text_document then
          text_document.diagnostic = nil
        end
      end,
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
      settings = { Lua = {} },
    },
  }

  ---@type table<string, vim.lsp.Config>
  local enabled = {}
  for _, name in ipairs(languages.list "servers") do
    if name ~= "gopls" or vim.fn.executable "go" == 1 then
      enabled[name] = servers[name] or {}
    end
  end
  return enabled
end

---@param servers table<string, vim.lsp.Config>
---@return string[]
function M.mason_tools(servers)
  local machine_tools = { biome = true, taplo = true, shucked = true }
  local ensure_installed = {}
  for name in pairs(servers) do
    if not machine_tools[name] then
      table.insert(ensure_installed, name)
    end
  end
  for _, tool in ipairs(languages.list "tools") do
    if tool ~= "goimports" or vim.fn.executable "go" == 1 then
      table.insert(ensure_installed, tool)
    end
  end
  table.sort(ensure_installed)
  return require("lib").unique(ensure_installed)
end

---@class LspAttachEvent: vim.api.keyset.create_autocmd.callback_args
---@field data { client_id: integer }

function M.toggle_inlay_hints()
  local bufnr = vim.api.nvim_get_current_buf()
  vim.lsp.inlay_hint.enable(not vim.lsp.inlay_hint.is_enabled { bufnr = bufnr }, { bufnr = bufnr })
end

---@param event vim.api.keyset.create_autocmd.callback_args
function M.clear_highlights(event)
  vim.lsp.buf.clear_references()
  local group = vim.api.nvim_create_augroup("lsp-highlight", { clear = false })
  vim.api.nvim_clear_autocmds { group = group, buffer = event.buf }
end

---@param bufnr integer
local function highlight_references(bufnr)
  local group = vim.api.nvim_create_augroup("lsp-highlight", { clear = false })
  vim.api.nvim_clear_autocmds { group = group, buffer = bufnr }
  vim.api.nvim_create_autocmd({ "CursorHold", "CursorHoldI" }, {
    buffer = bufnr,
    group = group,
    callback = vim.lsp.buf.document_highlight,
  })
  vim.api.nvim_create_autocmd({ "CursorMoved", "CursorMovedI" }, {
    buffer = bufnr,
    group = group,
    callback = vim.lsp.buf.clear_references,
  })
end

---@param mappings BufferMapping[]
---@return fun(event: LspAttachEvent)
function M.on_attach(mappings)
  return function(event)
    local client = vim.lsp.get_client_by_id(event.data.client_id)
    tooling.buffer_maps(event.buf, mappings, client)

    if client and client:supports_method("textDocument/documentHighlight", event.buf) then
      highlight_references(event.buf)
    end
  end
end

---@param mappings BufferMapping[]
function M.setup(mappings)
  vim.api.nvim_create_autocmd("LspAttach", {
    group = vim.api.nvim_create_augroup("lsp-attach", { clear = true }),
    callback = M.on_attach(mappings),
  })
  vim.api.nvim_create_autocmd("LspDetach", {
    group = vim.api.nvim_create_augroup("lsp-detach", { clear = true }),
    callback = M.clear_highlights,
  })
  local servers = M.configs()
  require("mason-tool-installer").setup { ensure_installed = M.mason_tools(servers) }
  for name, config in pairs(servers) do
    vim.lsp.config(name, config)
    vim.lsp.enable(name)
  end
end

return M
