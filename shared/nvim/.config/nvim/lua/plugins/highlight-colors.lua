return {
  {
    'brenoprata10/nvim-highlight-colors',
    event = 'BufReadPre',
    config = function()
      vim.opt.termguicolors = true

      require('nvim-highlight-colors').setup {
        render = 'virtual',
        virtual_symbol = '■',
        virtual_symbol_position = 'inline',

        enable_hex = true,
        enable_short_hex = true,
        enable_rgb = true,
        enable_hsl = true,

        enable_named_colors = false,
      }
    end,
  },
}
