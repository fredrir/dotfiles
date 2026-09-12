local M = {}

---@class SyntaxOptions
---@field highlight boolean
---@field indent boolean

---@param bufnr integer
---@param lang string
---@param opts SyntaxOptions
local function start(bufnr, lang, opts)
  if not vim.api.nvim_buf_is_valid(bufnr) or not vim.treesitter.language.add(lang) then
    return
  end
  if opts.highlight then
    vim.treesitter.start(bufnr, lang)
  end
  if opts.indent and vim.treesitter.query.get(lang, "indents") then
    vim.bo[bufnr].indentexpr = "v:lua.require'nvim-treesitter'.indentexpr()"
  end
end

---@param event vim.api.keyset.create_autocmd.callback_args
---@param opts SyntaxOptions
local function attach(event, opts)
  local lang = vim.treesitter.language.get_lang(event.match)
  if not lang then
    return
  end
  if vim.treesitter.language.add(lang) then
    start(event.buf, lang, opts)
    return
  end

  local ts = require "nvim-treesitter"
  if vim.tbl_contains(ts.get_available(), lang) then
    ts.install(lang):await(function(err)
      if not err then
        vim.schedule_wrap(start)(event.buf, lang, opts)
      end
    end)
  end
end

---@param opts SyntaxOptions
function M.setup(opts)
  local ts = require "nvim-treesitter"
  local installed = ts.get_installed()
  local missing = vim.tbl_filter(function(lang)
    return not vim.tbl_contains(installed, lang)
  end, require("languages").list "parsers")
  if #missing > 0 then
    ts.install(missing)
  end

  vim.api.nvim_create_autocmd("FileType", {
    group = vim.api.nvim_create_augroup("treesitter", { clear = true }),
    callback = function(event)
      attach(event, opts)
    end,
  })
end

return M
