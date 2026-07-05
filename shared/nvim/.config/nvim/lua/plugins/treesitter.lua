return {
  'nvim-treesitter/nvim-treesitter',
  lazy = false,
  branch = 'main',
  build = ':TSUpdate',
  config = function()
    local ts = require 'nvim-treesitter'

    -- On headless servers (vps/linux exports NVIM_MINIMAL) pre-warm only the
    -- shell/config/scripting/docs parsers and skip the web-frontend stack — the
    -- FileType autocmd below still installs anything else on demand. Neovim
    -- bundles c/lua/markdown/query/vim/vimdoc; the rest are compiled from C.
    local ensure = vim.g.minimal and {
      'bash', 'c', 'diff', 'go', 'json', 'lua', 'luadoc',
      'markdown', 'markdown_inline', 'python', 'query', 'vim', 'vimdoc', 'yaml',
    } or {
      'bash', 'c', 'css', 'diff', 'go', 'html', 'javascript', 'json',
      'lua', 'luadoc', 'markdown', 'markdown_inline', 'python', 'query',
      'tsx', 'typescript', 'vim', 'vimdoc', 'yaml',
    }
    local installed = ts.get_installed()
    local missing = vim.tbl_filter(function(lang)
      return not vim.tbl_contains(installed, lang)
    end, ensure)
    if #missing > 0 then
      ts.install(missing)
    end

    vim.api.nvim_create_autocmd('FileType', {
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
          vim.bo[buf].indentexpr = "v:lua.require'nvim-treesitter'.indentexpr()"
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
