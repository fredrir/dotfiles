local catalog = require "languages.catalog"

return {
  "nvim-treesitter/nvim-treesitter",
  lazy = false,
  branch = "main",
  build = ":TSUpdate",
  config = function()
    local ts = require "nvim-treesitter"

    local installed = ts.get_installed()
    local missing = vim.tbl_filter(function(lang)
      return not vim.tbl_contains(installed, lang)
    end, catalog)
    if #missing > 0 then
      ts.install(missing)
    end

    vim.api.nvim_create_autocmd("FileType", {
      callback = function(args)
        local buf = args.buf
        local lang = vim.treesitter.language.get_lang(args.match)
        if not lang then
          return
        end

        local function start()
          if not vim.treesitter.language.add(lang) then
            return
          end
          vim.treesitter.start(buf, lang)
          if vim.treesitter.query.get(lang, "indents") then
            vim.bo[buf].indentexpr = "v:lua.require'nvim-treesitter'.indentexpr()"
          end
        end

        if vim.treesitter.language.add(lang) then
          start()
        elseif vim.tbl_contains(ts.get_available(), lang) then
          ts.install(lang):await(function(err)
            if err then
              return
            end
            vim.schedule(function()
              if vim.api.nvim_buf_is_valid(buf) then
                start()
              end
            end)
          end)
        end
      end,
    })
  end,
}
