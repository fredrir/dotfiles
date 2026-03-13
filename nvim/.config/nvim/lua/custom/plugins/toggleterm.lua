return {
  'akinsho/toggleterm.nvim',
  version = '*',
  config = function()
    require('toggleterm').setup {
      size = 40,
      shade_filetypes = {},
      direction = 'vertical',
    }
  end,
}
