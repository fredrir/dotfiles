local parsers = {
  "bash",
  "c",
  "css",
  "diff",
  "go",
  "html",
  "javascript",
  "json",
  "lua",
  "luadoc",
  "markdown",
  "markdown_inline",
  "python",
  "query",
  "typescript",
  "tsx",
  "vim",
  "vimdoc",
  "yaml",
  "zsh",
}

---@type LazySpec
return {
  {
    "nvim-treesitter/nvim-treesitter",
    branch = "main",
    lazy = false,
    build = ":TSUpdate",
    config = function()
      local ts = require "nvim-treesitter"

      local installed = ts.get_installed()
      local missing = vim.tbl_filter(function(lang)
        return not vim.tbl_contains(installed, lang)
      end, parsers)
      if #missing > 0 then
        ts.install(missing)
      end

      ---@param bufnr integer
      ---@param lang string
      local function start(bufnr, lang)
        if not vim.api.nvim_buf_is_valid(bufnr) or not vim.treesitter.language.add(lang) then
          return
        end
        pcall(vim.treesitter.start, bufnr, lang)
        if vim.treesitter.query.get(lang, "indents") then
          vim.bo[bufnr].indentexpr = "v:lua.require'nvim-treesitter'.indentexpr()"
        end
      end

      vim.api.nvim_create_autocmd("FileType", {
        group = vim.api.nvim_create_augroup("treesitter", { clear = true }),
        callback = function(event)
          local lang = vim.treesitter.language.get_lang(event.match)
          if not lang then
            return
          end
          if vim.treesitter.language.add(lang) then
            start(event.buf, lang)
            return
          end
          if vim.tbl_contains(ts.get_available(), lang) then
            ts.install(lang):await(function(err)
              if not err then
                vim.schedule_wrap(start)(event.buf, lang)
              end
            end)
          end
        end,
      })
    end,
  },
}
