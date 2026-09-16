local tooling = require "utils.editor"
local languages = require "languages"
local M = {}

---@type FormatPreferences
local preferences

---@param opts conform.FormatOpts
function M.format(opts)
  -- Requiring Conform runs its Lazy setup before we read its preferences.
  local conform = require "conform"
  local bufnr = opts.bufnr or vim.api.nvim_get_current_buf()
  if languages.filetypes("shell")[vim.bo[bufnr].filetype] then
    opts = vim.tbl_extend("force", opts, preferences.shell, { bufnr = bufnr })
    local changedtick = vim.api.nvim_buf_get_changedtick(bufnr)
    conform.format(opts, function(err)
      if err and vim.api.nvim_buf_is_valid(bufnr) and vim.api.nvim_buf_get_changedtick(bufnr) ~= changedtick then
        return
      end
    end)
  else
    conform.format(opts)
  end
end

function M.buffer()
  require "conform"
  M.format(vim.deepcopy(preferences.manual))
end

---@param bufnr integer
---@return conform.FormatOpts? opts
---@return (fun(err: string?, did_edit: boolean?))? callback
function M.on_save(bufnr)
  local filetype = vim.bo[bufnr].filetype
  if (preferences.disabled_on_save or {})[filetype] then
    return nil
  end
  return (vim.deepcopy(preferences.on_save))
end

---@param opts FormatPreferences
function M.setup(opts)
  preferences = opts
  require("conform").setup {
    notify_on_error = opts.notify_on_error,
    format_on_save = M.on_save,
    formatters_by_ft = languages.by_filetype "formatters",
    formatters = {
      dotfmt = {
        command = "dotfmt",
        args = { "--stdin", "$FILENAME" },
        stdin = true,
      },
      jqfmt = {
        command = "jqfmt",
        args = { "--eq" },
        stdin = true,
      },
      stylua = {
        args = function(_, ctx)
          local args = { "--search-parent-directories", "--respect-ignores" }
          local config = tooling.fallback_config(ctx.buf, { ".stylua.toml", "stylua.toml" }, "stylua.toml")
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
          local config = tooling.fallback_config(ctx.buf, { ".taplo.toml", "taplo.toml" }, ".taplo.toml")
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
          local config = tooling.fallback_config(ctx.buf, { "biome.json", "biome.jsonc" }, "biome.global.json")
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
end

return M
